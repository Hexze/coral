#!/usr/bin/env python3
"""One-shot migration script: reads Urchin MongoDB, POSTs to Coral API."""

import sys
import requests
from collections import defaultdict
from pymongo import MongoClient
from datetime import datetime, timezone

CORAL_API = "https://api.urchin.gg/v3"
BATCH_SIZE = 200
SNAPSHOT_BATCH_SIZE = 1000

VALID_TAGS = {"sniper", "blatant_cheater", "closet_cheater", "confirmed_cheater"}

def parse_args():
    flags = set()
    api_key = None
    for a in sys.argv[1:]:
        if a.startswith("--"):
            flags.add(a)
        elif api_key is None:
            api_key = a
    return api_key, flags


def post(api_key, payload):
    r = requests.post(
        f"{CORAL_API}/migrate",
        json=payload,
        headers={"X-API-Key": api_key},
        timeout=300,
    )
    r.raise_for_status()
    return r.json()


def strip_uuid(val):
    if not val or not isinstance(val, str):
        return None
    return val.replace("-", "").lower()


def map_access_level(doc):
    if doc.get("is_admin"):
        return 4
    if doc.get("is_mod"):
        return 3
    if doc.get("private"):
        return 1
    return 0


def format_dt(val):
    if val is None:
        return None
    if isinstance(val, datetime):
        if val.tzinfo is None:
            val = val.replace(tzinfo=timezone.utc)
        return val.isoformat()
    if isinstance(val, str):
        if val.endswith("Z"):
            return val
        if "+" not in val and not val.endswith("Z"):
            return val + "+00:00"
        return val
    return None


def build_member(doc):
    mc_accounts = []
    for uuid in doc.get("minecraft_accounts", []) or []:
        cleaned = strip_uuid(uuid)
        if cleaned and len(cleaned) == 32:
            mc_accounts.append(cleaned)

    return {
        "discord_id": int(doc["discord_id"]),
        "uuid": strip_uuid(doc.get("uuid")),
        "join_date": format_dt(doc.get("join_date")),
        "request_count": doc.get("request_count", 0) or 0,
        "access_level": map_access_level(doc),
        "key_locked": doc.get("key_locked", False) or False,
        "config": doc.get("config") or {},
        "minecraft_accounts": mc_accounts,
    }


def map_tag(tag):
    tag_type = tag.get("tag_type", "")
    reason = tag.get("reason", "")
    added_by = tag.get("added_by")

    try:
        added_by = int(added_by)
    except (ValueError, TypeError):
        added_by = None

    if not added_by:
        return None

    if tag_type in ("caution", "account"):
        if "replays needed" in reason.lower():
            return {
                "tag_type": "replays_needed",
                "reason": "",
                "added_by": added_by,
                "added_on": format_dt(tag.get("added_on")),
                "hide_username": tag.get("hide_username", False) or False,
            }
        return None

    if tag_type not in VALID_TAGS:
        return None

    return {
        "tag_type": tag_type,
        "reason": reason,
        "added_by": added_by,
        "added_on": format_dt(tag.get("added_on")),
        "hide_username": tag.get("hide_username", False) or False,
    }


def collect_blacklist(db):
    """Merge duplicate UUID documents and deduplicate tags by type."""
    players = defaultdict(lambda: {"tags": [], "lock": None})

    for doc in db.blacklist.find():
        uuid = strip_uuid(doc.get("uuid"))
        if not uuid or len(uuid) != 32:
            continue

        entry = players[uuid]

        for tag in doc.get("tags", []) or []:
            mapped = map_tag(tag)
            if mapped:
                entry["tags"].append(mapped)

        if doc.get("is_locked") and not entry["lock"]:
            locked_at = None
            ts = doc.get("lock_timestamp")
            if isinstance(ts, datetime):
                if ts.tzinfo is None:
                    ts = ts.replace(tzinfo=timezone.utc)
                locked_at = ts.isoformat()

            locked_by = None
            raw = doc.get("locked_by")
            if raw:
                try:
                    locked_by = int(raw)
                except (ValueError, TypeError):
                    pass

            entry["lock"] = {
                "is_locked": True,
                "lock_reason": doc.get("lock_reason"),
                "locked_by": locked_by,
                "locked_at": locked_at,
                "evidence_thread": doc.get("evidence_thread"),
            }

    results = []
    for uuid, entry in players.items():
        seen_types = set()
        deduped = []
        for tag in entry["tags"]:
            if tag["tag_type"] not in seen_types:
                seen_types.add(tag["tag_type"])
                deduped.append(tag)

        if not deduped:
            continue

        lock = entry["lock"] or {}
        results.append({
            "uuid": uuid,
            "is_locked": lock.get("is_locked", False),
            "lock_reason": lock.get("lock_reason"),
            "locked_by": lock.get("locked_by"),
            "locked_at": lock.get("locked_at"),
            "evidence_thread": lock.get("evidence_thread"),
            "tags": deduped,
        })

    return results


def migrate_members(db, api_key):
    print("Migrating members...")
    batch = []
    total = 0
    errors = 0

    for doc in db.members.find():
        batch.append(build_member(doc))

        if len(batch) >= BATCH_SIZE:
            result = post(api_key, {"type": "members", "data": batch})
            total += result["migrated"]
            errors += result["errors"]
            print(f"  {total} members migrated ({errors} errors)")
            batch = []

    if batch:
        result = post(api_key, {"type": "members", "data": batch})
        total += result["migrated"]
        errors += result["errors"]

    print(f"Members done: {total} migrated, {errors} errors")


def migrate_blacklist(db, api_key):
    print("Collecting and merging blacklist...")
    players = collect_blacklist(db)
    print(f"  {len(players)} unique players with valid tags")

    total = 0
    errors = 0

    for i in range(0, len(players), BATCH_SIZE):
        batch = players[i:i + BATCH_SIZE]
        result = post(api_key, {"type": "blacklist", "data": batch})
        total += result["migrated"]
        errors += result["errors"]
        print(f"  {total} players migrated ({errors} errors)")

    print(f"Blacklist done: {total} migrated, {errors} errors")


CUTOFF_TIMESTAMP = 1774780408.0  # 2026-03-29T10:33:28 UTC — when V1 switched to coral

MODE_MAP = {
    "overall": "",
    "solos": "eight_one_",
    "doubles": "eight_two_",
    "threes": "four_three_",
    "fours": "four_four_",
    "4v4": "two_four_",
    "fourvfour": "two_four_",
}


def calculate_delta(old, new):
    if isinstance(old, dict) and isinstance(new, dict):
        delta = {}
        for key, new_val in new.items():
            if key in old:
                sub = calculate_delta(old[key], new_val)
                if sub is not None:
                    delta[key] = sub
            else:
                delta[key] = new_val
        return delta if delta else None
    if old == new:
        return None
    return new


def reverse_mode_stats(mode_data, prefix):
    """Convert V1 transformed mode stats back to flat Hypixel Bedwars keys."""
    if not mode_data:
        return {}
    bw = {}
    for key in ("wins", "losses", "final_kills", "final_deaths", "beds_broken", "beds_lost", "games_played"):
        val = mode_data.get(key, 0)
        if val and val != 0:
            bw[f"{prefix}{key}_bedwars"] = val

    ws = mode_data.get("winstreak")
    if ws is not None and ws != "?":
        ws_key = f"{prefix}winstreak" if prefix else "winstreak"
        bw[ws_key] = ws

    for group in ("kills", "deaths"):
        sub = mode_data.get(group, {})
        for sub_key, sub_val in sub.items():
            if sub_val and sub_val != 0:
                bw[f"{prefix}{sub_key}_bedwars"] = sub_val

    resources = mode_data.get("resources", {})
    resource_map = {
        "emeralds_collected": "emerald_resources_collected",
        "gold_collected": "gold_resources_collected",
        "diamonds_collected": "diamond_resources_collected",
        "iron_collected": "iron_resources_collected",
    }
    for rkey, hypixel_key in resource_map.items():
        val = resources.get(rkey, 0)
        if val and val != 0:
            bw[f"{prefix}{hypixel_key}_bedwars"] = val

    return bw


def reverse_transform(player_data):
    """Convert V1 transformed player data back to raw Hypixel player JSON."""
    if not player_data:
        return None
    bw_section = player_data.get("bedwars", {})
    modes = bw_section.get("modes", {})

    bedwars = {}
    exp = bw_section.get("experience", 0)
    if exp:
        bedwars["Experience"] = exp

    for mode_name, prefix in MODE_MAP.items():
        bedwars.update(reverse_mode_stats(modes.get(mode_name, {}), prefix))

    player = {
        "displayname": player_data.get("display_name"),
        "networkExp": player_data.get("network_exp"),
        "achievementPoints": player_data.get("achievement_points"),
        "firstLogin": player_data.get("first_login"),
        "lastLogin": player_data.get("last_login"),
        "lastLogout": player_data.get("last_logout"),
        "karma": player_data.get("karma"),
        "stats": {"Bedwars": bedwars},
    }

    ranks_gifted = player_data.get("ranks_gifted", 0)
    if ranks_gifted:
        player["giftingMeta"] = {"ranksGiven": ranks_gifted}

    stars = bw_section.get("stars")
    if stars is not None:
        try:
            star_val = int(str(stars).replace("✫", "").replace("✪", "").replace("⚝", "").strip())
            player["achievements"] = {"bedwars_level": star_val}
        except (ValueError, TypeError):
            pass

    return {k: v for k, v in player.items() if v is not None}


def collect_player_snapshots(uuid, entries):
    """Sort chronologically, reverse-transform, and delta-compress a single player's snapshots."""
    valid = []
    for entry in entries:
        ts = entry.get("timestamp")
        if not ts or ts >= CUTOFF_TIMESTAMP:
            continue
        player_data = entry.get("player")
        if player_data is None:
            continue
        raw = reverse_transform(player_data)
        if not raw:
            continue
        valid.append((ts, raw, player_data.get("display_name", "")))

    valid.sort(key=lambda x: x[0])

    snapshots = []
    previous = None
    for ts, transformed, username in valid:
        is_baseline = previous is None
        if is_baseline:
            data = transformed
        else:
            delta = calculate_delta(previous, transformed)
            if delta is None:
                continue
            data = delta

        iso_ts = datetime.fromtimestamp(ts, tz=timezone.utc).isoformat()
        snapshots.append({
            "uuid": uuid,
            "timestamp": iso_ts,
            "username": (username or "")[:16],
            "is_baseline": is_baseline,
            "data": data,
        })
        previous = transformed

    return snapshots


def flush_batch(api_key, batch, replace=False):
    total = 0
    errs = 0
    payload_type = "replace_snapshots" if replace else "snapshots"
    for i in range(0, len(batch), SNAPSHOT_BATCH_SIZE):
        chunk = batch[i:i + SNAPSHOT_BATCH_SIZE]
        try:
            result = post(api_key, {"type": payload_type, "data": chunk})
            total += result["migrated"]
            errs += result["errors"]
        except Exception as e:
            print(f"  Batch error: {e}")
            errs += len(chunk)
    return total, errs


PLAYER_BATCH_SIZE = 2000

def migrate_snapshots(db, api_key, replace=False):
    mode = "Replacing" if replace else "Migrating"
    print(f"{mode} snapshots...")
    batch = []
    total = 0
    errors = 0
    players = 0

    for doc in db.cache.find({}, no_cursor_timeout=True).batch_size(100):
        uuid = strip_uuid(doc.get("uuid"))
        if not uuid or len(uuid) != 32:
            continue

        snapshots = collect_player_snapshots(uuid, doc.get("data", []) or [])
        batch.extend(snapshots)
        players += 1

        if players % PLAYER_BATCH_SIZE == 0:
            t, e = flush_batch(api_key, batch, replace)
            total += t
            errors += e
            batch = []
            print(f"  {players} players ({total} snapshots, {errors} errors)")

    if batch:
        t, e = flush_batch(api_key, batch, replace)
        total += t
        errors += e

    print(f"Snapshots done: {total} snapshots across {players} players ({errors} errors)")


def main():
    api_key, flags = parse_args()
    actions = flags & {"--members", "--blacklist", "--cache", "--snapshots", "--replace-snapshots"}
    if not api_key or not actions:
        print("Usage: python3 migrate_to_coral.py <INTERNAL_API_KEY> [--wipe] [--members] [--blacklist] [--cache] [--snapshots] [--replace-snapshots]")
        print("  --wipe               Wipe data before migrating")
        print("  --members            Migrate members")
        print("  --blacklist          Migrate blacklist")
        print("  --cache              Wipe cache (snapshots + sessions)")
        print("  --snapshots          Migrate V1 cache to coral snapshots")
        print("  --replace-snapshots  Re-migrate snapshots, deleting coral-originated ones per player")
        sys.exit(1)

    db = MongoClient("mongodb://localhost:27017").urchindb

    if "--cache" in flags:
        print("Wiping cache...")
        print(f"  {post(api_key, {'type': 'wipe_cache'})}")

    if "--members" in flags:
        if "--wipe" in flags:
            print("Wiping members...")
            print(f"  {post(api_key, {'type': 'wipe_members'})}")
        migrate_members(db, api_key)

    if "--blacklist" in flags:
        if "--wipe" in flags:
            print("Wiping blacklist...")
            print(f"  {post(api_key, {'type': 'wipe_blacklist'})}")
        migrate_blacklist(db, api_key)

    if "--replace-snapshots" in flags:
        migrate_snapshots(db, api_key, replace=True)
    elif "--snapshots" in flags:
        migrate_snapshots(db, api_key)

    print("Done!")


if __name__ == "__main__":
    main()
