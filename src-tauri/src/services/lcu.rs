use std::collections::HashMap;

use base64::Engine;
use reqwest::{header::CONTENT_TYPE, Client, StatusCode};
use serde::{de::DeserializeOwned, Serialize};

use super::error::{AppError, AppResult};
use super::models::{
    ChampSelectSession, ChampionSummaryItem, ChatMe, ClientAuth, GameAssetBundle, GameAssetEntry,
    GameflowSession, GiftableFriend, LcuFriend, MatchHistoryResponse, RankedStatsResponse,
    RiotAlias, SummonerInfo,
};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntitlementsToken {
    #[serde(default)]
    access_token: String,
}

fn basic_auth_header(token: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("riot:{token}"));
    format!("Basic {encoded}")
}

fn https_client() -> AppResult<Client> {
    // LCU/RC 使用本地自签名证书。只允许访问 127.0.0.1，证书校验在这里放宽。
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .timeout(std::time::Duration::from_secs(14))
        .build()
        .map_err(AppError::Http)
}

#[derive(Clone)]
pub struct LcuClient {
    http: Client,
    base_url: String,
    auth_header: String,
}

impl LcuClient {
    pub fn new(auth: &ClientAuth) -> AppResult<Self> {
        Ok(Self {
            http: https_client()?,
            base_url: format!("https://127.0.0.1:{}", auth.port),
            auth_header: basic_auth_header(&auth.auth_token),
        })
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> AppResult<T> {
        let response = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .header("Authorization", &self.auth_header)
            .send()
            .await?;

        if response.status() == StatusCode::NOT_FOUND {
            return Err(AppError::PlayerNotFound(path.to_string()));
        }

        if response.status() == StatusCode::UNPROCESSABLE_ENTITY {
            return Err(lcu_unprocessable_entity_error(path));
        }

        if !response.status().is_success() {
            return Err(AppError::LcuUnavailable(format!(
                "{} 返回 {}",
                path,
                response.status()
            )));
        }

        response.json::<T>().await.map_err(AppError::Http)
    }

    async fn get_optional_json<T: DeserializeOwned>(&self, path: &str) -> AppResult<Option<T>> {
        let response = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .header("Authorization", &self.auth_header)
            .send()
            .await?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if response.status() == StatusCode::UNPROCESSABLE_ENTITY {
            return Err(lcu_unprocessable_entity_error(path));
        }

        if !response.status().is_success() {
            return Err(AppError::LcuUnavailable(format!(
                "{} 返回 {}",
                path,
                response.status()
            )));
        }

        response.json::<T>().await.map(Some).map_err(AppError::Http)
    }

    async fn get_bytes(&self, path: &str) -> AppResult<(Vec<u8>, String)> {
        let response = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .header("Authorization", &self.auth_header)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::LcuUnavailable(format!(
                "{} 返回 {}",
                path,
                response.status()
            )));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("image/png")
            .to_string();
        let bytes = response.bytes().await?.to_vec();

        Ok((bytes, content_type))
    }

    async fn post_empty(&self, path: &str) -> AppResult<()> {
        let response = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("Authorization", &self.auth_header)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::LcuUnavailable(format!(
                "{} 返回 {}",
                path,
                response.status()
            )));
        }

        Ok(())
    }

    async fn put_json<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> AppResult<()> {
        let response = self
            .http
            .put(format!("{}{}", self.base_url, path))
            .header("Authorization", &self.auth_header)
            .json(body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::LcuUnavailable(format!(
                "{} 返回 {}",
                path,
                response.status()
            )));
        }

        Ok(())
    }

    pub async fn ping(&self) -> AppResult<()> {
        let _: serde_json::Value = self.get_json("/riotclient/auth-token").await?;
        Ok(())
    }

    pub async fn entitlements_access_token(&self) -> AppResult<String> {
        let token: EntitlementsToken = self.get_json("/entitlements/v1/token").await?;
        if token.access_token.trim().is_empty() {
            return Err(AppError::LcuUnavailable(
                "entitlements token 为空，无法读取深度战绩".to_string(),
            ));
        }

        Ok(token.access_token)
    }

    pub async fn league_session_token(&self) -> AppResult<String> {
        let token: String = self
            .get_json("/lol-league-session/v1/league-session-token")
            .await?;
        if token.trim().is_empty() {
            return Err(AppError::LcuUnavailable(
                "league session token 为空，无法读取段位".to_string(),
            ));
        }

        Ok(token)
    }

    pub async fn current_summoner(&self) -> AppResult<SummonerInfo> {
        self.get_json("/lol-summoner/v1/current-summoner").await
    }

    pub async fn summoner_by_puuid(&self, puuid: &str) -> AppResult<SummonerInfo> {
        self.get_json(&format!("/lol-summoner/v2/summoners/puuid/{puuid}"))
            .await
    }

    pub async fn summoner_by_name(&self, name: &str) -> AppResult<SummonerInfo> {
        let encoded = urlencoding::encode(name);
        self.get_json(&format!("/lol-summoner/v1/summoners?name={encoded}"))
            .await
    }

    pub async fn ranked_stats(&self, puuid: &str) -> AppResult<RankedStatsResponse> {
        self.get_json(&format!("/lol-ranked/v1/ranked-stats/{puuid}"))
            .await
    }

    pub async fn gameflow_phase(&self) -> AppResult<String> {
        self.get_json("/lol-gameflow/v1/gameflow-phase").await
    }

    pub async fn accept_ready_check(&self) -> AppResult<()> {
        self.post_empty("/lol-matchmaking/v1/ready-check/accept")
            .await
    }

    pub async fn dismiss_end_of_game(&self) -> AppResult<()> {
        self.post_empty("/lol-end-of-game/v1/state/dismiss-stats")
            .await
    }

    pub async fn chat_me(&self) -> AppResult<ChatMe> {
        self.get_json("/lol-chat/v1/me").await
    }

    pub async fn set_chat_availability(&self, availability: &str) -> AppResult<()> {
        self.put_json(
            "/lol-chat/v1/me",
            &serde_json::json!({ "availability": availability }),
        )
        .await
    }

    pub async fn set_chat_status_message(&self, status_message: &str) -> AppResult<()> {
        self.put_json(
            "/lol-chat/v1/me",
            &serde_json::json!({ "statusMessage": status_message }),
        )
        .await
    }

    pub async fn friends(&self) -> AppResult<Vec<LcuFriend>> {
        self.get_json("/lol-chat/v1/friends").await
    }

    pub async fn giftable_friends(&self) -> AppResult<Vec<GiftableFriend>> {
        self.get_json("/lol-store/v1/giftablefriends").await
    }

    pub async fn install_dir(&self) -> AppResult<String> {
        self.get_json("/data-store/v1/install-dir").await
    }

    pub async fn gameflow_session(&self) -> AppResult<Option<GameflowSession>> {
        self.get_optional_json("/lol-gameflow/v1/session").await
    }

    pub async fn champ_select_session(&self) -> AppResult<Option<ChampSelectSession>> {
        self.get_optional_json("/lol-champ-select/v1/session").await
    }

    pub async fn match_history(
        &self,
        puuid: &str,
        begin_index: usize,
        end_index: usize,
    ) -> AppResult<MatchHistoryResponse> {
        self.get_json(&format!(
            "/lol-match-history/v1/products/lol/{puuid}/matches?begIndex={begin_index}&endIndex={end_index}"
        ))
        .await
    }

    pub async fn match_history_game(&self, game_id: u64) -> AppResult<super::models::Game> {
        self.get_json(&format!("/lol-match-history/v1/games/{game_id}"))
            .await
    }

    pub async fn champion_summary(&self) -> AppResult<Vec<ChampionSummaryItem>> {
        let mut champions: Vec<ChampionSummaryItem> = self
            .get_json("/lol-game-data/assets/v1/champion-summary.json")
            .await?;

        for champion in &mut champions {
            if champion.id <= 0 || !champion.title.is_empty() {
                continue;
            }

            let path = format!("/lol-game-data/assets/v1/champions/{}.json", champion.id);
            if let Ok(detail) = self.get_json::<serde_json::Value>(&path).await {
                champion.title = read_string_field(&detail, &["title", "titleTRA", "titleCn"]);
            }
        }

        // LCU 的 champion-summary 第一项通常是 id=-1 的 None 占位项。
        // 前端统计只需要真实英雄，过滤掉占位项也能避免无效数据进入映射表。
        Ok(champions
            .into_iter()
            .filter(|champion| champion.id > 0)
            .collect())
    }

    pub async fn game_assets(&self) -> AppResult<GameAssetBundle> {
        let summoner_spells = self
            .load_game_asset_entries("/lol-game-data/assets/v1/summoner-spells.json")
            .await;
        let items = self
            .load_game_asset_entries("/lol-game-data/assets/v1/items.json")
            .await;
        let mut perks = self
            .load_game_asset_entries("/lol-game-data/assets/v1/perks.json")
            .await;
        let augments = self
            .load_game_asset_entries("/lol-game-data/assets/v1/cherry-augments.json")
            .await;

        for augment in &augments {
            if !perks.iter().any(|perk| perk.id == augment.id) {
                perks.push(augment.clone());
            }
        }

        Ok(GameAssetBundle {
            summoner_spells,
            items,
            perks,
            augments,
        })
    }

    async fn load_game_asset_entries(&self, path: &str) -> Vec<GameAssetEntry> {
        let Ok(entries) = self.get_json::<Vec<serde_json::Value>>(path).await else {
            return Vec::new();
        };

        entries
            .into_iter()
            .filter_map(game_asset_entry_from_value)
            .collect()
    }

    pub async fn asset_data_url(&self, path: &str) -> AppResult<String> {
        let path = normalize_lcu_asset_path(path)?;
        let (bytes, content_type) = self.get_bytes(&path).await?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        Ok(format!("data:{content_type};base64,{encoded}"))
    }

    pub async fn asset_data_urls(&self, paths: Vec<String>) -> AppResult<HashMap<String, String>> {
        let mut result = HashMap::new();

        for path in paths {
            if result.contains_key(&path) {
                continue;
            }

            if let Ok(data_url) = self.asset_data_url(&path).await {
                result.insert(path, data_url);
            }
        }

        Ok(result)
    }
}

fn game_asset_entry_from_value(value: serde_json::Value) -> Option<GameAssetEntry> {
    let id = read_u32_field(&value, "id")?;
    if id == 0 {
        return None;
    }

    Some(GameAssetEntry {
        id,
        name: read_string_field(&value, &["name", "nameTRA", "nameCn", "displayName"]),
        description: read_string_field(&value, &["description", "longDesc", "shortDesc"]),
        icon_path: read_string_field(
            &value,
            &[
                "iconPath",
                "augmentSmallIconPath",
                "iconSmall",
                "iconLargePath",
                "iconLarge",
            ],
        ),
        rarity: read_rarity_field(&value),
        categories: read_string_vec_field(&value, "categories"),
        price: read_u32_field(&value, "price").unwrap_or_default(),
        price_total: read_u32_field(&value, "priceTotal").unwrap_or_default(),
        in_store: read_bool_field(&value, "inStore"),
        display_in_item_sets: read_bool_field(&value, "displayInItemSets"),
    })
}

fn read_u32_field(value: &serde_json::Value, field: &str) -> Option<u32> {
    let raw = value.get(field)?;
    raw.as_u64()
        .and_then(|id| u32::try_from(id).ok())
        .or_else(|| raw.as_str()?.parse::<u32>().ok())
}

fn lcu_unprocessable_entity_error(path: &str) -> AppError {
    if path.starts_with("/lol-summoner/v1/summoners?name=") {
        return AppError::InvalidInput(
            "召唤师名称格式不正确。Riot ID 请使用半角 #，例如 ivy#49164".to_string(),
        );
    }

    AppError::InvalidInput(format!("客户端不接受这个请求参数：{path}"))
}

fn read_string_field(value: &serde_json::Value, fields: &[&str]) -> String {
    fields
        .iter()
        .find_map(|field| value.get(*field)?.as_str())
        .map(str::to_string)
        .unwrap_or_default()
}

fn read_string_vec_field(value: &serde_json::Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(|raw| raw.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn read_bool_field(value: &serde_json::Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(|raw| {
            raw.as_bool().or_else(|| {
                raw.as_str()
                    .map(|text| matches!(text.to_ascii_lowercase().as_str(), "true" | "1"))
            })
        })
        .unwrap_or(false)
}

fn read_rarity_field(value: &serde_json::Value) -> String {
    let Some(raw) = value.get("rarity") else {
        return String::new();
    };

    if let Some(rarity) = raw.as_str() {
        return rarity.to_string();
    }

    let rarity_index = raw
        .as_u64()
        .or_else(|| raw.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| {
            raw.as_f64()
                .filter(|value| value.is_finite())
                .map(|value| value as u64)
        });

    match rarity_index {
        Some(0) => "kSilver".to_string(),
        Some(1) => "kGold".to_string(),
        Some(2) => "kPrismatic".to_string(),
        Some(3) => "kBronze".to_string(),
        _ => String::new(),
    }
}

fn normalize_lcu_asset_path(path: &str) -> AppResult<String> {
    let path = path.trim();
    if !path.starts_with("/lol-game-data/assets/") || path.contains("..") {
        return Err(AppError::LcuUnavailable(
            "拒绝读取非 lol-game-data 资源路径".to_string(),
        ));
    }

    Ok(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::client_discovery::discover_primary_client;
    use crate::services::stats::{load_match_detail, load_player_stats};
    use base64::Engine;
    use serde_json::Value;
    use std::collections::HashSet;

    fn contains_rating_term(text: &str) -> bool {
        let normalized = text.to_ascii_lowercase();
        [
            "mmr",
            "elo",
            "rating",
            "rank",
            "league",
            "lp",
            "skill",
            "matchmaking",
        ]
        .iter()
        .any(|term| normalized.contains(term))
    }

    fn collect_help_hits(value: &Value, path: &str, hits: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let next_path = if path.is_empty() {
                        key.to_string()
                    } else {
                        format!("{path}.{key}")
                    };
                    if contains_rating_term(key) {
                        hits.push(next_path.clone());
                    }
                    collect_help_hits(child, &next_path, hits);
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    collect_help_hits(child, &format!("{path}[{index}]"), hits);
                }
            }
            Value::String(text) => {
                if contains_rating_term(text) {
                    hits.push(format!("{path}={text}"));
                }
            }
            _ => {}
        }
    }

    fn search_json_targets(value: &Value, path: &str, needles: &[&str], hits: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let next_path = if path.is_empty() {
                        key.to_string()
                    } else {
                        format!("{path}.{key}")
                    };
                    let key_lower = key.to_ascii_lowercase();
                    if needles
                        .iter()
                        .any(|needle| key_lower.contains(&needle.to_ascii_lowercase()))
                    {
                        hits.push(format!("{next_path} => {}", child));
                    }
                    search_json_targets(child, &next_path, needles, hits);
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    search_json_targets(child, &format!("{path}[{index}]"), needles, hits);
                }
            }
            Value::String(text) => {
                let text_lower = text.to_ascii_lowercase();
                if needles
                    .iter()
                    .any(|needle| text_lower.contains(&needle.to_ascii_lowercase()))
                {
                    hits.push(format!("{path} => {text}"));
                }
            }
            _ => {}
        }
    }

    fn decode_jwt_payload(jwt: &str) -> Option<(Value, Value)> {
        let mut parts = jwt.split('.');
        let header = parts.next()?;
        let payload = parts.next()?;
        let header_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(header.as_bytes())
            .ok()?;
        let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.as_bytes())
            .ok()?;
        let header_json = serde_json::from_slice::<Value>(&header_bytes).ok()?;
        let payload_json = serde_json::from_slice::<Value>(&payload_bytes).ok()?;
        Some((header_json, payload_json))
    }

    #[tokio::test]
    #[ignore = "手动触发：探测本机 LCU /help 是否暴露 MMR/ELO 相关接口"]
    async fn manual_probe_lcu_help_for_rating_endpoints() {
        let auth = discover_primary_client().expect("应能找到已登录客户端");
        let lcu = LcuClient::new(&auth).expect("应能创建 LCU 客户端");

        let response = lcu
            .http
            .get(format!("{}{}", lcu.base_url, "/help"))
            .header("Authorization", &lcu.auth_header)
            .send()
            .await
            .expect("/help 请求应成功");

        let status = response.status();
        let text = response.text().await.expect("应能读取 /help 文本");
        println!("lcu /help status => {}", status);

        let json: Value = serde_json::from_str(&text).expect("/help 应返回 JSON");
        let mut hits = Vec::new();
        collect_help_hits(&json, "", &mut hits);
        hits.sort();
        hits.dedup();

        println!("lcu /help rating-like hits => {:?}", hits);
    }

    #[tokio::test]
    #[ignore = "手动触发：探测 LCU 候选 ranked/matchmaking 端点的实际返回"]
    async fn manual_probe_lcu_candidate_rating_endpoints() {
        let auth = discover_primary_client().expect("应能找到已登录客户端");
        let lcu = LcuClient::new(&auth).expect("应能创建 LCU 客户端");
        let candidates = [
            "/lol-ranked/v1/current-ranked-stats",
            "/lol-ranked/v1/signed-ranked-stats",
            "/lol-ranked/v1/current-lp-change-notification",
            "/lol-platform-config/v1/namespaces/QueueRestriction",
            "/lol-ranked/v1/top-rated-ladders-enabled",
            "/lol-ranked/v1/challenger-ladders-enabled",
        ];

        for path in candidates {
            let response = lcu
                .http
                .get(format!("{}{}", lcu.base_url, path))
                .header("Authorization", &lcu.auth_header)
                .send()
                .await
                .unwrap_or_else(|error| panic!("{path} 请求失败: {error}"));

            let status = response.status();
            let text = response.text().await.expect("应能读取文本");
            println!("\nendpoint => {path}");
            println!("status => {status}");

            let preview = if text.len() > 1800 {
                format!("{}...(truncated)", &text[..1800])
            } else {
                text.clone()
            };
            println!("body => {preview}");

            if let Ok(json) = serde_json::from_str::<Value>(&text) {
                let mut hits = Vec::new();
                collect_help_hits(&json, "", &mut hits);
                hits.sort();
                hits.dedup();
                println!("rating-like json hits => {:?}", hits);
            }
        }
    }

    #[tokio::test]
    #[ignore = "手动触发：从 LCU /help 定位 ranked ladder 和 leaderboard 候选端点"]
    async fn manual_probe_lcu_help_ranked_targets() {
        let auth = discover_primary_client().expect("应能找到已登录客户端");
        let lcu = LcuClient::new(&auth).expect("应能创建 LCU 客户端");
        let response = lcu
            .http
            .get(format!("{}{}", lcu.base_url, "/help"))
            .header("Authorization", &lcu.auth_header)
            .send()
            .await
            .expect("/help 请求应成功");
        let text = response.text().await.expect("应能读取 /help 文本");
        let json: Value = serde_json::from_str(&text).expect("/help 应返回 JSON");

        let needles = [
            "GetLolRankedV1LeagueLaddersByPuuid",
            "GetLolRankedV1RatedLadderByQueueType",
            "GetLolRankedV1SocialLeaderboardRankedQueueStatsForPuuids",
            "GetLolRankedV1ApexLeaguesByQueueTypeByTier",
            "/lol-ranked/v1/league-ladders",
            "/lol-ranked/v1/rated-ladder",
            "/lol-ranked/v1/social-leaderboard",
        ];
        let mut hits = Vec::new();
        search_json_targets(&json, "", &needles, &mut hits);
        hits.sort();
        hits.dedup();

        for hit in hits {
            println!("help target => {hit}");
        }
    }

    #[tokio::test]
    #[ignore = "手动触发：解码 signed-ranked-stats 的 JWT 载荷"]
    async fn manual_probe_signed_ranked_stats_jwt() {
        let auth = discover_primary_client().expect("应能找到已登录客户端");
        let lcu = LcuClient::new(&auth).expect("应能创建 LCU 客户端");
        let response = lcu
            .http
            .get(format!(
                "{}{}",
                lcu.base_url, "/lol-ranked/v1/signed-ranked-stats"
            ))
            .header("Authorization", &lcu.auth_header)
            .send()
            .await
            .expect("signed-ranked-stats 请求应成功");
        let text = response.text().await.expect("应能读取返回文本");
        let json: Value = serde_json::from_str(&text).expect("signed-ranked-stats 应返回 JSON");
        let jwt = json
            .get("jwt")
            .and_then(Value::as_str)
            .expect("返回里应包含 jwt");
        let (header, payload) = decode_jwt_payload(jwt).expect("jwt 应能正常解码");

        println!("signed-ranked-stats jwt header => {header}");
        println!("signed-ranked-stats jwt payload => {payload}");

        let mut hits = Vec::new();
        search_json_targets(
            &payload,
            "",
            &["mmr", "elo", "rating", "rank", "league", "lp", "skill"],
            &mut hits,
        );
        hits.sort();
        hits.dedup();

        println!("signed-ranked-stats jwt rating-like hits => {:?}", hits);
    }

    #[tokio::test]
    #[ignore = "手动触发：实探 ranked ladder 和 leaderboard 候选端点"]
    async fn manual_probe_ranked_ladder_candidates() {
        let auth = discover_primary_client().expect("应能找到已登录客户端");
        let lcu = LcuClient::new(&auth).expect("应能创建 LCU 客户端");
        let summoner = lcu.current_summoner().await.expect("应能读取当前召唤师");
        let puuid = summoner.puuid;

        let candidates = [
            format!("/lol-ranked/v1/league-ladders/{puuid}"),
            format!("/lol-ranked/v1/league-ladders/by-puuid/{puuid}"),
            "/lol-ranked/v1/rated-ladder/RANKED_SOLO_5x5".to_string(),
            "/lol-ranked/v1/rated-ladder/RANKED_TFT".to_string(),
            "/lol-ranked/v1/apex-leagues/RANKED_SOLO_5x5/CHALLENGER".to_string(),
            "/lol-ranked/v1/apex-leagues/RANKED_TFT/CHALLENGER".to_string(),
            format!("/lol-ranked/v1/social-leaderboard/ranked-queue-stats-for-puuids?puuids={puuid}"),
            format!("/lol-ranked/v1/social-leaderboard/ranked-queue-stats-for-puuids?queueType=RANKED_SOLO_5x5&puuids={puuid}"),
            format!("/lol-ranked/v1/social-leaderboard/ranked-queue-stats-for-puuids?queueType=RANKED_TFT&puuids={puuid}"),
        ];

        for path in candidates {
            let response = lcu
                .http
                .get(format!("{}{}", lcu.base_url, path))
                .header("Authorization", &lcu.auth_header)
                .send()
                .await
                .unwrap_or_else(|error| panic!("{path} 请求失败: {error}"));
            let status = response.status();
            let text = response.text().await.expect("应能读取文本");
            let preview = if text.len() > 1200 {
                format!("{}...(truncated)", &text[..1200])
            } else {
                text
            };
            println!("\ncandidate => {path}");
            println!("status => {status}");
            println!("body => {preview}");
        }
    }

    fn extract_queue_probe(value: &Value) -> Vec<String> {
        let Some(queues) = value.get("queues").and_then(Value::as_array) else {
            return Vec::new();
        };

        queues
            .iter()
            .filter_map(|queue| {
                let queue_type = queue.get("queueType").and_then(Value::as_str).unwrap_or("");
                let tier = queue.get("tier").and_then(Value::as_str).unwrap_or("");
                let rank = queue
                    .get("rank")
                    .or_else(|| queue.get("division"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let league_points = queue
                    .get("leaguePoints")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let cumulative_lp = queue
                    .get("cumulativeLp")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let rated_rating = queue
                    .get("ratedRating")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let wins = queue
                    .get("wins")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let losses = queue
                    .get("losses")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();

                let has_signal = !queue_type.is_empty()
                    && (!tier.is_empty()
                        || rated_rating != 0
                        || cumulative_lp != 0
                        || league_points != 0
                        || wins != 0
                        || losses != 0);
                if !has_signal {
                    return None;
                }

                Some(format!(
                    "{} tier={} rank={} lp={} cumulativeLp={} ratedRating={} w/l={}/{}",
                    queue_type,
                    tier,
                    rank,
                    league_points,
                    cumulative_lp,
                    rated_rating,
                    wins,
                    losses
                ))
            })
            .collect()
    }

    async fn load_raw_ranked_probe(lcu: &LcuClient, puuid: &str) -> Value {
        let response = lcu
            .http
            .get(format!(
                "{}{}",
                lcu.base_url,
                format!("/lol-ranked/v1/ranked-stats/{puuid}")
            ))
            .header("Authorization", &lcu.auth_header)
            .send()
            .await
            .expect("ranked-stats 请求应成功");
        let text = response.text().await.expect("应能读取 ranked-stats 文本");
        serde_json::from_str(&text).expect("ranked-stats 应返回 JSON")
    }

    #[tokio::test]
    #[ignore = "手动触发：对比当前账号单人开局与组队开局对局里疑似分数字段"]
    async fn manual_compare_solo_and_premade_rating_fields() {
        let auth = discover_primary_client().expect("应能找到已登录客户端");
        let lcu = LcuClient::new(&auth).expect("应能创建 LCU 客户端");
        let summoner = lcu.current_summoner().await.expect("应能读取当前召唤师");

        let stats = load_player_stats(&lcu, Some(&auth), None, summoner.clone(), 80)
            .await
            .expect("应能读取当前战绩");
        let solo_game = stats
            .recent_games
            .iter()
            .find(|game| game.premade_teammate_puuids.is_empty())
            .cloned()
            .expect("最近 80 局里至少应有一局单人开局");
        let premade_game = stats
            .recent_games
            .iter()
            .find(|game| !game.premade_teammate_puuids.is_empty())
            .cloned()
            .expect("最近 80 局里至少应有一局组队开局");

        for (label, game, premades) in [
            ("solo", solo_game, HashSet::<String>::new()),
            (
                "premade",
                premade_game.clone(),
                premade_game
                    .premade_teammate_puuids
                    .iter()
                    .map(|puuid| puuid.to_ascii_lowercase())
                    .collect::<HashSet<_>>(),
            ),
        ] {
            println!(
                "\n===== {} game_id={} queue_id={} mode={} premades={:?} =====",
                label, game.game_id, game.queue_id, game.game_mode, game.premade_teammate_puuids
            );

            let detail = load_match_detail(&lcu, Some(&auth), None, game.game_id)
                .await
                .expect("应能读取对局详情");

            let mut rated_values = Vec::new();
            let mut cumulative_values = Vec::new();

            for team in detail.teams {
                for player in team.players {
                    let raw_ranked = load_raw_ranked_probe(&lcu, &player.puuid).await;
                    let queues = extract_queue_probe(&raw_ranked);
                    let rated_rating = raw_ranked
                        .get("highestRankedEntry")
                        .and_then(|value| value.get("ratedRating"))
                        .and_then(Value::as_i64)
                        .unwrap_or_default();
                    let cumulative_lp = raw_ranked
                        .get("highestRankedEntry")
                        .and_then(|value| value.get("cumulativeLp"))
                        .and_then(Value::as_i64)
                        .unwrap_or_default();
                    if rated_rating != 0 {
                        rated_values.push(rated_rating);
                    }
                    if cumulative_lp != 0 {
                        cumulative_values.push(cumulative_lp);
                    }

                    let is_owner = player.puuid.eq_ignore_ascii_case(&summoner.puuid);
                    let is_same_team = player.team_id == game.team_id;
                    let is_premade = premades.contains(&player.puuid.to_ascii_lowercase());
                    println!(
                        "player => owner={} sameTeam={} premade={} team={} subteam={} riot='{}#{}' summoner='{}' probes={:?}",
                        is_owner,
                        is_same_team,
                        is_premade,
                        player.team_id,
                        player.player_subteam_id,
                        player.game_name,
                        player.tag_line,
                        player.summoner_name,
                        queues
                    );
                }
            }

            println!(
                "{} summary => nonZeroRatedRating={:?} nonZeroCumulativeLp={:?}",
                label, rated_values, cumulative_values
            );
        }
    }
}

#[derive(Clone)]
pub struct RiotClient {
    http: Client,
    base_url: String,
    auth_header: String,
}

impl RiotClient {
    pub fn new(auth: &ClientAuth) -> AppResult<Option<Self>> {
        let Some(port) = auth.riot_client_port else {
            return Ok(None);
        };

        let Some(token) = &auth.riot_client_auth_token else {
            return Ok(None);
        };

        Ok(Some(Self {
            http: https_client()?,
            base_url: format!("https://127.0.0.1:{port}"),
            auth_header: basic_auth_header(token),
        }))
    }

    pub async fn lookup_alias(
        &self,
        game_name: &str,
        tag_line: Option<&str>,
    ) -> AppResult<Vec<RiotAlias>> {
        let mut request = self
            .http
            .get(format!(
                "{}/player-account/aliases/v1/lookup",
                self.base_url
            ))
            .header("Authorization", &self.auth_header)
            .query(&[("gameName", game_name)]);

        if let Some(tag_line) = tag_line {
            request = request.query(&[("tagLine", tag_line)]);
        }

        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(AppError::RiotClientUnavailable(format!(
                "alias lookup 返回 {}",
                response.status()
            )));
        }

        response
            .json::<Vec<RiotAlias>>()
            .await
            .map_err(AppError::Http)
    }
}
