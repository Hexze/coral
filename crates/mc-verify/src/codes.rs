use reqwest::Client;
use uuid::Uuid;

const CODE_MIN: u16 = 1000;
const CODE_MAX: u16 = 9999;
const MAX_ATTEMPTS: usize = 100;

#[derive(Clone)]
pub struct CodeStore {
    http: Client,
    api_url: String,
    api_key: String,
}

impl CodeStore {
    pub fn new(http: Client, api_url: String, api_key: String) -> Self {
        Self {
            http,
            api_url,
            api_key,
        }
    }

    pub async fn insert(&self, uuid: Uuid, username: String) -> String {
        let url = format!("{}/v3/verify/codes", self.api_url);

        for _ in 0..MAX_ATTEMPTS {
            let code = generate_code();
            let response = self
                .http
                .post(&url)
                .header("X-API-Key", &self.api_key)
                .json(&serde_json::json!({
                    "code": code,
                    "uuid": uuid.simple().to_string(),
                    "username": username,
                }))
                .send()
                .await
                .expect("failed to store verification code");

            if response.status().is_success() {
                return code;
            }
        }

        panic!("failed to generate unique code after {MAX_ATTEMPTS} attempts");
    }
}

fn generate_code() -> String {
    let n: u16 = CODE_MIN + (rand::random::<u16>() % (CODE_MAX - CODE_MIN + 1));
    n.to_string()
}
