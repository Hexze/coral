use serde::Serialize;
use utoipa::ToSchema;


#[derive(Serialize, ToSchema)]
pub struct PlayerStatsResponse {
    pub uuid: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<serde_json::Value>)]
    pub hypixel: Option<serde_json::Value>,
    pub tags: Vec<TagResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skin_url: Option<String>,
}


#[derive(Serialize, ToSchema)]
pub struct PlayerTagsResponse {
    pub uuid: String,
    pub tags: Vec<TagResponse>,
}


#[derive(Serialize, ToSchema)]
pub struct TagResponse {
    pub id: i64,
    pub tag_type: String,
    pub reason: String,
    pub added_by: i64,
    pub added_on: String,
    pub hide_username: bool,
}


#[derive(Serialize, ToSchema)]
pub struct CubelifyResponse {
    pub score: CubelifyScore,
    pub tags: Vec<CubelifyTag>,
}


#[derive(Serialize, ToSchema)]
pub struct CubelifyScore {
    pub value: f64,
    pub mode: &'static str,
}


#[derive(Serialize, ToSchema)]
pub struct CubelifyTag {
    pub icon: String,
    pub color: u32,
    pub tooltip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}


impl CubelifyResponse {
    pub fn error(message: &str, icon: &str) -> Self {
        Self {
            score: CubelifyScore { value: 0.0, mode: "add" },
            tags: vec![CubelifyTag {
                icon: icon.to_string(),
                color: 0xFF0000,
                tooltip: message.to_string(),
                text: None,
            }],
        }
    }
}


#[derive(Serialize, ToSchema)]
pub struct SuccessResponse {
    pub success: bool,
}


impl TagResponse {
    pub fn from_db(tag: &database::PlayerTagRow) -> Self {
        Self {
            id: tag.id,
            tag_type: tag.tag_type.clone(),
            reason: tag.reason.clone(),
            added_by: tag.added_by,
            added_on: tag.added_on.to_rfc3339(),
            hide_username: tag.hide_username,
        }
    }
}
