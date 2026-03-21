use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

const CODE_TTL: Duration = Duration::from_secs(120);
const CODE_MIN: u16 = 1000;
const CODE_MAX: u16 = 9999;
const MAX_ATTEMPTS: usize = 100;

pub struct CodeStore {
    by_code: Mutex<HashMap<String, Entry>>,
}

struct Entry {
    uuid: Uuid,
    username: String,
    expires_at: Instant,
}

pub struct VerifiedPlayer {
    pub uuid: Uuid,
    pub username: String,
}

impl CodeStore {
    pub fn new() -> Self {
        Self {
            by_code: Mutex::new(HashMap::new()),
        }
    }

    pub fn insert(&self, uuid: Uuid, username: String) -> String {
        let mut map = self.by_code.lock().unwrap();
        self.purge_expired(&mut map);

        if let Some((code, entry)) = map.iter_mut().find(|(_, e)| e.uuid == uuid) {
            entry.expires_at = Instant::now() + CODE_TTL;
            return code.clone();
        }

        let code = generate_unique_code(&map);
        map.insert(
            code.clone(),
            Entry {
                uuid,
                username,
                expires_at: Instant::now() + CODE_TTL,
            },
        );
        code
    }

    pub fn redeem(&self, code: &str) -> Option<VerifiedPlayer> {
        let mut map = self.by_code.lock().unwrap();
        let entry = map.remove(code)?;

        if entry.expires_at < Instant::now() {
            return None;
        }

        Some(VerifiedPlayer {
            uuid: entry.uuid,
            username: entry.username,
        })
    }

    fn purge_expired(&self, map: &mut HashMap<String, Entry>) {
        let now = Instant::now();
        map.retain(|_, e| e.expires_at > now);
    }
}

fn generate_unique_code(existing: &HashMap<String, Entry>) -> String {
    for _ in 0..MAX_ATTEMPTS {
        let n: u16 = CODE_MIN + (rand::random::<u16>() % (CODE_MAX - CODE_MIN + 1));
        let code = n.to_string();
        if !existing.contains_key(&code) {
            return code;
        }
    }
    panic!("failed to generate unique code after {MAX_ATTEMPTS} attempts");
}
