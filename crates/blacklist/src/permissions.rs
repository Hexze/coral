const MEMBER: i16 = 1;
const HELPER: i16 = 2;
const MODERATOR: i16 = 3;

const WINDOW_MINUTES: i64 = 30;


pub fn can_add(tag_type: &str, level: i16) -> bool {
    match tag_type {
        "sniper" => true,
        "blatant_cheater" | "closet_cheater" => level >= MEMBER,
        "replays_needed" => level >= HELPER,
        "caution" => level >= MODERATOR,
        _ => false,
    }
}


pub fn can_remove(tag_type: &str, level: i16, is_own: bool, age_minutes: i64) -> bool {
    if level >= MODERATOR { return true; }
    if level >= HELPER {
        return tag_type != "confirmed_cheater" && tag_type != "caution";
    }
    is_own && age_minutes <= WINDOW_MINUTES
}


pub fn can_modify(tag_type: &str, level: i16, is_own: bool, age_minutes: i64) -> bool {
    can_remove(tag_type, level, is_own, age_minutes)
}


pub fn can_change_to(new_type: &str, level: i16) -> bool {
    can_add(new_type, level)
}


pub fn can_set_hide(level: i16) -> bool {
    level >= MODERATOR
}
