use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;

use super::error::{AppError, AppResult};
use super::lcu::LcuClient;
use super::models::{
    ActiveGamePlayer, ActiveGameTeam, ClientAuth, Game, GameTeam, IdentityPlayer,
    OfficialApiDebugResponse, OfficialApiProbeSample, Participant, ParticipantIdentity,
    ParticipantStats, ParticipantTimeline, PlayerActiveGame, RankedQueueEntry, RankedQueueMap,
    RankedStatsResponse, SummonerInfo,
};

const USER_AGENT: &str = "LeagueOfLegendsClient/14.13.596.7996 (rcp-be-lol-match-history)";
const MAX_RETRIES: usize = 3;
const OFFICIAL_PROBE_SAMPLE_COUNT: usize = 3;

/// 国服 SGP（Service Gateway Proxy）深战绩客户端。
///
/// LCU 的 `/lol-match-history/v1/products/lol/{puuid}/matches` 在部分国服客户端上会忽略
/// `begIndex/endIndex`，只能稳定拿到第一页。Akari 的深分页走 SGP，这里只实现统计需要的
/// SUMMARY 接口，并把返回值映射成项目内部已有的 `Game` 结构。
pub struct SgpClient {
    http: Client,
    base_url: String,
    access_token: String,
    league_session_token: String,
    sgp_server_id: String,
}

impl SgpClient {
    pub async fn new(
        lcu: &LcuClient,
        auth: &ClientAuth,
        sgp_server_id: Option<&str>,
    ) -> AppResult<Self> {
        let sgp_server_id = resolve_sgp_server_id(auth, sgp_server_id);
        let base_url = tencent_sgp_base_url(&sgp_server_id)?;
        let access_token = lcu.entitlements_access_token().await?;
        let league_session_token = lcu
            .league_session_token()
            .await
            .unwrap_or_else(|_| access_token.clone());
        let http = Client::builder()
            .danger_accept_invalid_certs(true)
            .no_proxy()
            .timeout(std::time::Duration::from_secs(14))
            .build()
            .map_err(AppError::Http)?;

        Ok(Self {
            http,
            base_url: base_url.to_string(),
            access_token,
            league_session_token,
            sgp_server_id,
        })
    }

    pub async fn ranked_stats(&self, puuid: &str) -> AppResult<RankedStatsResponse> {
        let response = self
            .http
            .get(format!(
                "{}/leagues-ledge/v2/rankedStats/puuid/{}",
                self.base_url, puuid
            ))
            .header(
                "Authorization",
                format!("Bearer {}", self.league_session_token),
            )
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;

        if response.status() == StatusCode::NOT_FOUND {
            return Err(AppError::PlayerNotFound(puuid.to_string()));
        }

        if !response.status().is_success() {
            return Err(AppError::SgpUnavailable(format!(
                "SGP 段位返回 {}",
                response.status()
            )));
        }

        let ranked = parse_sgp_json::<SgpRankedStats>(response, "SGP 段位").await?;

        Ok(sgp_ranked_stats_to_ranked_response(ranked))
    }

    pub async fn summoners_by_puuids(&self, puuids: &[String]) -> AppResult<Vec<SummonerInfo>> {
        if puuids.is_empty() {
            return Ok(Vec::new());
        }

        let sub_id = self
            .sgp_server_id
            .strip_prefix("TENCENT_")
            .unwrap_or(&self.sgp_server_id)
            .to_ascii_lowercase();

        let response = self
            .http
            .post(format!(
                "{}/summoner-ledge/v1/regions/{}/summoners/puuids",
                self.base_url, sub_id
            ))
            .header(
                "Authorization",
                format!("Bearer {}", self.league_session_token),
            )
            .header("User-Agent", USER_AGENT)
            .json(puuids)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::SgpUnavailable(format!(
                "SGP 召唤师查询返回 {}",
                response.status()
            )));
        }

        let summoners = parse_sgp_json::<Vec<SgpSummoner>>(response, "SGP 召唤师").await?;
        Ok(summoners.into_iter().map(sgp_summoner_to_lcu).collect())
    }

    pub async fn match_history(
        &self,
        puuid: &str,
        start_index: usize,
        count: usize,
    ) -> AppResult<Vec<Game>> {
        let mut last_status = None;
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            let response = self
                .http
                .get(format!(
                    "{}/match-history-query/v1/products/lol/player/{}/SUMMARY",
                    self.base_url, puuid
                ))
                .header("Authorization", format!("Bearer {}", self.access_token))
                .header("User-Agent", USER_AGENT)
                .query(&[
                    ("startIndex", start_index.to_string()),
                    ("count", count.to_string()),
                ])
                .send()
                .await;

            let response = match response {
                Ok(response) => response,
                Err(error) if attempt < MAX_RETRIES => {
                    last_error = Some(error);
                    wait_before_retry(attempt).await;
                    continue;
                }
                Err(error) => return Err(AppError::Http(error)),
            };

            if should_retry_status(response.status()) && attempt < MAX_RETRIES {
                last_status = Some(response.status());
                wait_before_retry(attempt).await;
                continue;
            }

            return self.parse_match_history_response(response, puuid).await;
        }

        if let Some(status) = last_status {
            return Err(AppError::SgpUnavailable(format!(
                "SGP 深战绩多次返回 {}",
                status
            )));
        }

        if let Some(error) = last_error {
            return Err(AppError::SgpUnavailable(format!(
                "SGP 深战绩网络请求失败：{}",
                error
            )));
        }

        Err(AppError::SgpUnavailable(
            "SGP 深战绩请求未返回结果".to_string(),
        ))
    }

    pub async fn game_summary(&self, game_id: u64) -> AppResult<Game> {
        let sub_id = self
            .sgp_server_id
            .strip_prefix("TENCENT_")
            .unwrap_or(&self.sgp_server_id)
            .to_ascii_uppercase();

        let response = self
            .http
            .get(format!(
                "{}/match-history-query/v1/products/lol/{}_{}/SUMMARY",
                self.base_url, sub_id, game_id
            ))
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;

        self.parse_game_summary_response(response, game_id).await
    }

    async fn parse_match_history_response(
        &self,
        response: reqwest::Response,
        puuid: &str,
    ) -> AppResult<Vec<Game>> {
        if response.status() == StatusCode::NOT_FOUND {
            return Err(AppError::PlayerNotFound(puuid.to_string()));
        }

        if !response.status().is_success() {
            return Err(AppError::SgpUnavailable(format!(
                "SGP 深战绩返回 {}",
                response.status()
            )));
        }

        let history = parse_sgp_json::<SgpMatchHistoryResponse>(response, "SGP 深战绩").await?;

        Ok(history
            .games
            .into_iter()
            .filter_map(|game| game.json.map(sgp_game_json_to_lcu_game))
            .collect())
    }

    async fn parse_game_summary_response(
        &self,
        response: reqwest::Response,
        game_id: u64,
    ) -> AppResult<Game> {
        if response.status() == StatusCode::NOT_FOUND {
            return Err(AppError::PlayerNotFound(game_id.to_string()));
        }

        if !response.status().is_success() {
            return Err(AppError::SgpUnavailable(format!(
                "SGP 对局详情返回 {}",
                response.status()
            )));
        }

        let summary = parse_sgp_json::<SgpGameEnvelope>(response, "SGP 对局详情").await?;

        let Some(summary) = summary.json else {
            return Err(AppError::SgpUnavailable(format!(
                "SGP 对局详情 {game_id} 缺少 json 数据"
            )));
        };

        Ok(sgp_game_json_to_lcu_game(summary))
    }

    /// 查询指定账号当前是否在游戏中（SGP 观战服务 `gsm/v1/ledge/spectator`）。
    ///
    /// 返回 404 表示该账号当前没有进行中的对局；成功则返回双方阵容与英雄信息。
    pub async fn active_game_by_puuid(&self, puuid: &str) -> AppResult<PlayerActiveGame> {
        let sub_id = self
            .sgp_server_id
            .strip_prefix("TENCENT_")
            .unwrap_or(&self.sgp_server_id)
            .to_ascii_uppercase();

        let request_url = format!(
            "{}/gsm/v1/ledge/spectator/region/{}/puuid/{}",
            self.base_url, sub_id, puuid
        );

        let send = |token: &str| {
            self.http
                .get(&request_url)
                .header("Authorization", format!("Bearer {token}"))
                .header("User-Agent", USER_AGENT)
                .send()
        };

        let mut response = send(&self.league_session_token).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            // 观战接口可能只认 entitlements token，换 access_token 重试一次。
            response = send(&self.access_token).await?;
        }

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(PlayerActiveGame {
                in_game: false,
                game_id: None,
                queue_config_id: None,
                queue_type_name: None,
                game_mode: None,
                game_state: None,
                teams: Vec::new(),
            });
        }

        if !response.status().is_success() {
            return Err(AppError::SgpUnavailable(format!(
                "SGP 实时对局返回 {}",
                response.status()
            )));
        }

        let info = parse_sgp_json::<SgpActiveGame>(response, "SGP 实时对局").await?;
        Ok(sgp_active_game_to_response(info))
    }
}

pub async fn debug_official_api_probe(
    lcu: &LcuClient,
    auth: &ClientAuth,
    sgp_server_id: Option<&str>,
) -> AppResult<OfficialApiDebugResponse> {
    let summoner = lcu.current_summoner().await?;
    let sgp = SgpClient::new(lcu, auth, sgp_server_id).await?;
    let sub_id = sgp
        .sgp_server_id
        .strip_prefix("TENCENT_")
        .unwrap_or(&sgp.sgp_server_id)
        .to_ascii_uppercase();

    let ranked_request_url = format!(
        "{}/leagues-ledge/v2/rankedStats/puuid/{}",
        sgp.base_url, summoner.puuid
    );
    let ranked_payload = sgp
        .http
        .get(&ranked_request_url)
        .header(
            "Authorization",
            format!("Bearer {}", sgp.league_session_token),
        )
        .header("User-Agent", USER_AGENT)
        .send()
        .await?
        .text()
        .await
        .map_err(AppError::Http)?;
    let ranked_like_paths = collect_rank_like_paths_from_text(&ranked_payload);

    let match_history_request_url = format!(
        "{}/match-history-query/v1/products/lol/player/{}/SUMMARY?startIndex=0&count={}",
        sgp.base_url, summoner.puuid, OFFICIAL_PROBE_SAMPLE_COUNT
    );
    let match_history_payload = sgp
        .http
        .get(format!(
            "{}/match-history-query/v1/products/lol/player/{}/SUMMARY",
            sgp.base_url, summoner.puuid
        ))
        .header("Authorization", format!("Bearer {}", sgp.access_token))
        .header("User-Agent", USER_AGENT)
        .query(&[
            ("startIndex", "0".to_string()),
            ("count", OFFICIAL_PROBE_SAMPLE_COUNT.to_string()),
        ])
        .send()
        .await?
        .text()
        .await
        .map_err(AppError::Http)?;
    let match_history_like_paths = collect_rank_like_paths_from_text(&match_history_payload);

    let games = sgp
        .match_history(&summoner.puuid, 0, OFFICIAL_PROBE_SAMPLE_COUNT)
        .await
        .unwrap_or_default();
    let owner_champion_by_game = games
        .iter()
        .map(|game| {
            let champion_id = game
                .participant_identities
                .iter()
                .find(|identity| identity.player.puuid.eq_ignore_ascii_case(&summoner.puuid))
                .and_then(|identity| {
                    game.participants
                        .iter()
                        .find(|participant| participant.participant_id == identity.participant_id)
                        .map(|participant| participant.champion_id)
                })
                .unwrap_or_default();
            (game.game_id, champion_id)
        })
        .collect::<std::collections::HashMap<_, _>>();

    let mut sample_games = Vec::new();
    for game in games.into_iter().take(OFFICIAL_PROBE_SAMPLE_COUNT) {
        let summary_request_url = format!(
            "{}/match-history-query/v1/products/lol/{}_{}/SUMMARY",
            sgp.base_url, sub_id, game.game_id
        );
        let summary_payload = sgp
            .http
            .get(&summary_request_url)
            .header("Authorization", format!("Bearer {}", sgp.access_token))
            .header("User-Agent", USER_AGENT)
            .send()
            .await?
            .text()
            .await
            .map_err(AppError::Http)?;

        sample_games.push(OfficialApiProbeSample {
            game_id: game.game_id,
            queue_id: game.queue_id,
            game_mode: game.game_mode,
            game_creation: game.game_creation,
            champion_id: owner_champion_by_game
                .get(&game.game_id)
                .copied()
                .unwrap_or_default(),
            summary_request_url,
            rank_like_paths: collect_rank_like_paths_from_text(&summary_payload),
            first_participant_challenge_hits: collect_first_participant_challenge_hits(&summary_payload),
            summary_payload,
        });
    }

    Ok(OfficialApiDebugResponse {
        sgp_server_id: sgp.sgp_server_id,
        base_url: sgp.base_url,
        summoner,
        ranked_request_url,
        ranked_payload,
        ranked_like_paths,
        match_history_request_url,
        match_history_payload,
        match_history_like_paths,
        sample_games,
        note: "展示的是当前客户端可直接访问的官方接口原始返回，不做二次美化。字段命中会额外标出包含 elo/mmr/rating/rank/tier 等关键词的路径。".to_string(),
    })
}

pub fn default_sgp_server_id(auth: &ClientAuth) -> String {
    if auth.region.eq_ignore_ascii_case("TENCENT") {
        format!("TENCENT_{}", auth.rso_platform_id.to_ascii_uppercase())
    } else {
        auth.region.to_ascii_uppercase()
    }
}

pub fn resolve_sgp_server_id(auth: &ClientAuth, sgp_server_id: Option<&str>) -> String {
    sgp_server_id
        .filter(|value| !value.trim().is_empty())
        .map(normalize_sgp_server_id)
        .unwrap_or_else(|| default_sgp_server_id(auth))
}

pub fn normalize_sgp_server_id(sgp_server_id: &str) -> String {
    sgp_server_id.trim().to_ascii_uppercase()
}

#[derive(Debug, Default, Deserialize)]
struct SgpMatchHistoryResponse {
    #[serde(default)]
    games: Vec<SgpGameEnvelope>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpRankedStats {
    #[serde(default)]
    queues: Vec<SgpRankedQueue>,
    #[serde(default)]
    queue_map: Option<SgpRankedQueueMap>,
}

#[derive(Debug, Default, Deserialize)]
struct SgpRankedQueueMap {
    #[serde(default, rename = "RANKED_SOLO_5x5", alias = "RANKED_SOLO_5X5")]
    ranked_solo_5x5: SgpRankedQueue,
    #[serde(default, rename = "RANKED_FLEX_SR")]
    ranked_flex_sr: SgpRankedQueue,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpRankedQueue {
    #[serde(default)]
    queue_type: String,
    #[serde(default)]
    tier: String,
    #[serde(default, alias = "division")]
    rank: String,
    #[serde(default)]
    league_points: i32,
    #[serde(default)]
    wins: i32,
    #[serde(default)]
    losses: i32,
    #[serde(default)]
    provisional_games_remaining: i32,
    #[serde(default)]
    highest_tier: String,
    #[serde(default, alias = "highestDivision")]
    highest_rank: String,
    #[serde(default)]
    previous_season_end_tier: String,
    #[serde(default, alias = "previousSeasonEndDivision")]
    previous_season_end_rank: String,
    #[serde(default)]
    previous_season_highest_tier: String,
    #[serde(default, alias = "previousSeasonHighestDivision")]
    previous_season_highest_rank: String,
    #[serde(default)]
    previous_season_achieved_tier: String,
    #[serde(default, alias = "previousSeasonAchievedDivision")]
    previous_season_achieved_rank: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpSummoner {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    puuid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    internal_name: String,
    #[serde(default)]
    profile_icon_id: u32,
    #[serde(default)]
    level: u32,
    #[serde(default)]
    privacy: String,
}

#[derive(Debug, Default, Deserialize)]
struct SgpGameEnvelope {
    #[serde(default)]
    json: Option<SgpGameJson>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpGameJson {
    #[serde(default)]
    game_id: u64,
    #[serde(default)]
    game_creation: i64,
    #[serde(default)]
    game_duration: i64,
    #[serde(default)]
    game_mode: String,
    #[serde(default)]
    game_type: String,
    #[serde(default)]
    queue_id: u32,
    #[serde(default)]
    participants: Vec<SgpParticipant>,
    #[serde(default)]
    teams: Vec<SgpTeam>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpTeam {
    #[serde(default)]
    team_id: u32,
    #[serde(default)]
    objectives: SgpObjectives,
}

#[derive(Debug, Default, Deserialize)]
struct SgpObjectives {
    #[serde(default)]
    tower: SgpObjective,
}

#[derive(Debug, Default, Deserialize)]
struct SgpObjective {
    #[serde(default)]
    kills: u32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpParticipant {
    #[serde(default)]
    puuid: String,
    #[serde(default)]
    riot_id_game_name: String,
    #[serde(default)]
    riot_id_tagline: String,
    #[serde(default)]
    summoner_name: String,
    #[serde(default)]
    participant_id: u32,
    #[serde(default)]
    champion_id: u32,
    #[serde(default)]
    spell1_id: u32,
    #[serde(default)]
    spell2_id: u32,
    #[serde(default)]
    team_id: u32,
    #[serde(default)]
    win: bool,
    #[serde(default)]
    kills: u32,
    #[serde(default)]
    deaths: u32,
    #[serde(default)]
    assists: u32,
    #[serde(default)]
    turret_kills: u32,
    #[serde(default)]
    gold_earned: u32,
    #[serde(default)]
    total_minions_killed: u32,
    #[serde(default)]
    neutral_minions_killed: u32,
    #[serde(default)]
    total_damage_dealt_to_champions: u32,
    #[serde(default)]
    damage_self_mitigated: u32,
    #[serde(default)]
    total_damage_taken: u32,
    #[serde(default)]
    total_heal: u32,
    #[serde(default)]
    vision_score: u32,
    #[serde(default)]
    champ_level: u32,
    #[serde(default)]
    player_subteam_id: u32,
    #[serde(default)]
    challenges: Value,
    #[serde(default)]
    item0: u32,
    #[serde(default)]
    item1: u32,
    #[serde(default)]
    item2: u32,
    #[serde(default)]
    item3: u32,
    #[serde(default)]
    item4: u32,
    #[serde(default)]
    item5: u32,
    #[serde(default)]
    item6: u32,
    #[serde(default)]
    perk0: u32,
    #[serde(default)]
    perk1: u32,
    #[serde(default)]
    perk2: u32,
    #[serde(default)]
    perk3: u32,
    #[serde(default)]
    perk4: u32,
    #[serde(default)]
    perk5: u32,
    #[serde(default)]
    player_augment1: u32,
    #[serde(default)]
    player_augment2: u32,
    #[serde(default)]
    player_augment3: u32,
    #[serde(default)]
    player_augment4: u32,
    #[serde(default)]
    player_augment5: u32,
    #[serde(default)]
    player_augment6: u32,
    #[serde(default)]
    lane: String,
    #[serde(default)]
    role: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpActiveGame {
    #[serde(default)]
    game: Option<SgpActiveGameDetail>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpActiveGameDetail {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    game_state: String,
    #[serde(default)]
    queue_type_name: String,
    #[serde(default)]
    game_mode: String,
    #[serde(default)]
    game_queue_config_id: i32,
    #[serde(default)]
    team_one: Vec<SgpActivePlayer>,
    #[serde(default)]
    team_two: Vec<SgpActivePlayer>,
    #[serde(default)]
    player_champion_selections: Vec<SgpActiveSelection>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpActivePlayer {
    #[serde(default)]
    puuid: String,
    // SGP 观战接口返回的召唤师数字 ID；当前下游以 puuid 为主键，此字段仅随反序列化保留
    // 以备后续需要 summonerId 映射时使用，故允许 dead_code。
    #[serde(default)]
    #[allow(dead_code)]
    summoner_id: i64,
    #[serde(default)]
    profile_icon_id: u32,
    #[serde(default)]
    champion_id: u32,
    #[serde(default)]
    summoner_name: String,
    #[serde(default)]
    summoner_internal_name: String,
    #[serde(default)]
    selected_role: String,
    #[serde(default)]
    selected_position: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpActiveSelection {
    #[serde(default)]
    summoner_internal_name: String,
    #[serde(default)]
    summoner_name: String,
    #[serde(default)]
    spell1_id: u32,
    #[serde(default)]
    spell2_id: u32,
}

async fn parse_sgp_json<T>(response: reqwest::Response, context: &str) -> AppResult<T>
where
    T: DeserializeOwned,
{
    let body = response.text().await.map_err(AppError::Http)?;
    let mut deserializer = serde_json::Deserializer::from_str(&body);

    serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let path = error.path().to_string();
        let path = if path.trim().is_empty() {
            "<root>".to_string()
        } else {
            path
        };

        AppError::SgpUnavailable(format!(
            "{context}解析失败：路径 {path}，{}；响应片段：{}",
            error.inner(),
            response_preview(&body)
        ))
    })
}

fn response_preview(body: &str) -> String {
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview = compact.chars().take(240).collect::<String>();

    if compact.chars().count() > 240 {
        format!("{preview}...")
    } else if preview.is_empty() {
        "<empty>".to_string()
    } else {
        preview
    }
}

fn challenge_u32(challenges: &Value, key: &str) -> u32 {
    challenges
        .get(key)
        .and_then(json_value_to_u32)
        .unwrap_or_default()
}

fn json_value_to_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_i64().and_then(|value| u64::try_from(value).ok()))
            .or_else(|| number.as_f64().map(|value| value.max(0.0).round() as u64))
            .and_then(|value| u32::try_from(value).ok()),
        Value::String(value) => value.trim().parse::<u32>().ok().or_else(|| {
            value
                .trim()
                .parse::<f64>()
                .ok()
                .map(|value| value.max(0.0).round() as u32)
        }),
        _ => None,
    }
}

fn sgp_game_json_to_lcu_game(game: SgpGameJson) -> Game {
    let identities = game
        .participants
        .iter()
        .map(|participant| ParticipantIdentity {
            participant_id: participant.participant_id,
            player: IdentityPlayer {
                puuid: participant.puuid.clone(),
                game_name: participant.riot_id_game_name.clone(),
                tag_line: participant.riot_id_tagline.clone(),
                summoner_name: participant.summoner_name.clone(),
            },
        })
        .collect();

    let participants = game
        .participants
        .into_iter()
        .map(|participant| {
            let enemy_champion_immobilizations =
                challenge_u32(&participant.challenges, "enemyChampionImmobilizations");
            let immobilize_and_kill_with_ally =
                challenge_u32(&participant.challenges, "immobilizeAndKillWithAlly");

            Participant {
                participant_id: participant.participant_id,
                champion_id: participant.champion_id,
                spell1_id: participant.spell1_id,
                spell2_id: participant.spell2_id,
                team_id: participant.team_id,
                stats: ParticipantStats {
                    win: participant.win,
                    kills: participant.kills,
                    deaths: participant.deaths,
                    assists: participant.assists,
                    turret_kills: participant.turret_kills,
                    gold_earned: participant.gold_earned,
                    total_minions_killed: participant.total_minions_killed,
                    neutral_minions_killed: participant.neutral_minions_killed,
                    total_damage_dealt_to_champions: participant.total_damage_dealt_to_champions,
                    damage_self_mitigated: participant.damage_self_mitigated,
                    total_damage_taken: participant.total_damage_taken,
                    total_heal: participant.total_heal,
                    enemy_champion_immobilizations,
                    immobilize_and_kill_with_ally,
                    vision_score: participant.vision_score,
                    champ_level: participant.champ_level,
                    player_subteam_id: participant.player_subteam_id,
                    item0: participant.item0,
                    item1: participant.item1,
                    item2: participant.item2,
                    item3: participant.item3,
                    item4: participant.item4,
                    item5: participant.item5,
                    item6: participant.item6,
                    perk0: participant.perk0,
                    perk1: participant.perk1,
                    perk2: participant.perk2,
                    perk3: participant.perk3,
                    perk4: participant.perk4,
                    perk5: participant.perk5,
                    player_augment1: participant.player_augment1,
                    player_augment2: participant.player_augment2,
                    player_augment3: participant.player_augment3,
                    player_augment4: participant.player_augment4,
                    player_augment5: participant.player_augment5,
                    player_augment6: participant.player_augment6,
                },
                timeline: ParticipantTimeline {
                    lane: participant.lane,
                    role: participant.role,
                },
            }
        })
        .collect();

    let teams = game
        .teams
        .into_iter()
        .map(|team| GameTeam {
            team_id: team.team_id,
            tower_kills: team.objectives.tower.kills,
        })
        .collect();

    Game {
        game_id: game.game_id,
        game_creation: game.game_creation,
        game_duration: game.game_duration,
        game_mode: game.game_mode,
        game_type: game.game_type,
        queue_id: game.queue_id,
        participant_identities: identities,
        participants,
        teams,
    }
}

fn sgp_summoner_to_lcu(summoner: SgpSummoner) -> SummonerInfo {
    SummonerInfo {
        display_name: summoner.name,
        internal_name: summoner.internal_name,
        profile_icon_id: summoner.profile_icon_id,
        puuid: summoner.puuid,
        summoner_id: summoner.id,
        summoner_level: summoner.level,
        privacy: summoner.privacy,
        ..Default::default()
    }
}

fn sgp_active_game_to_response(info: SgpActiveGame) -> PlayerActiveGame {
    let Some(game) = info.game else {
        return PlayerActiveGame {
            in_game: false,
            game_id: None,
            queue_config_id: None,
            queue_type_name: None,
            game_mode: None,
            game_state: None,
            teams: Vec::new(),
        };
    };

    // 技能只出现在 playerChampionSelections 里，按召唤师内部名配对到各队成员。
    let spells = game
        .player_champion_selections
        .iter()
        .map(|selection| {
            (
                active_player_key(&selection.summoner_internal_name, &selection.summoner_name),
                selection.spell1_id,
                selection.spell2_id,
            )
        })
        .collect::<Vec<_>>();

    let build_team = |players: Vec<SgpActivePlayer>, name: String| ActiveGameTeam {
        name,
        players: players
            .into_iter()
            .map(|player| {
                let key =
                    active_player_key(&player.summoner_internal_name, &player.summoner_name);
                let (spell1_id, spell2_id) = spells
                    .iter()
                    .find(|(entry, _, _)| *entry == key)
                    .map(|(_, spell1, spell2)| (*spell1, *spell2))
                    .unwrap_or((0, 0));
                ActiveGamePlayer {
                    puuid: player.puuid,
                    summoner_name: player.summoner_name,
                    champion_id: player.champion_id,
                    spell1_id,
                    spell2_id,
                    profile_icon_id: player.profile_icon_id,
                    position: if player.selected_position.trim().is_empty() {
                        player.selected_role
                    } else {
                        player.selected_position
                    },
                }
            })
            .collect(),
    };

    PlayerActiveGame {
        in_game: true,
        game_id: Some(game.id),
        queue_config_id: Some(game.game_queue_config_id),
        queue_type_name: Some(game.queue_type_name),
        game_mode: Some(game.game_mode),
        game_state: Some(game.game_state),
        teams: vec![
            build_team(game.team_one, "蓝方".to_string()),
            build_team(game.team_two, "红方".to_string()),
        ],
    }
}

fn active_player_key(internal_name: &str, summoner_name: &str) -> String {
    if internal_name.trim().is_empty() {
        summoner_name.trim().to_string()
    } else {
        internal_name.trim().to_string()
    }
}

fn sgp_ranked_stats_to_ranked_response(ranked: SgpRankedStats) -> RankedStatsResponse {
    let mut queue_map = RankedQueueMap::default();

    if let Some(raw_queue_map) = ranked.queue_map {
        if sgp_ranked_queue_has_data(&raw_queue_map.ranked_solo_5x5) {
            queue_map.ranked_solo_5x5 =
                sgp_ranked_queue_to_entry(&raw_queue_map.ranked_solo_5x5, "RANKED_SOLO_5x5");
        }
        if sgp_ranked_queue_has_data(&raw_queue_map.ranked_flex_sr) {
            queue_map.ranked_flex_sr =
                sgp_ranked_queue_to_entry(&raw_queue_map.ranked_flex_sr, "RANKED_FLEX_SR");
        }
    }

    for queue in ranked.queues {
        let queue_type = normalize_ranked_queue_type(&queue.queue_type);
        if !sgp_ranked_queue_has_data(&queue) {
            continue;
        }
        let entry = sgp_ranked_queue_to_entry(&queue, queue.queue_type.trim());
        match queue_type.as_str() {
            "RANKED_SOLO_5x5" => queue_map.ranked_solo_5x5 = entry,
            "RANKED_FLEX_SR" => queue_map.ranked_flex_sr = entry,
            _ => {}
        }
    }

    RankedStatsResponse { queue_map }
}

fn sgp_ranked_queue_has_data(queue: &SgpRankedQueue) -> bool {
    ranked_text_has_value(&queue.tier)
        || ranked_text_has_value(&queue.highest_tier)
        || ranked_text_has_value(&queue.previous_season_end_tier)
        || ranked_text_has_value(&queue.previous_season_highest_tier)
        || ranked_text_has_value(&queue.previous_season_achieved_tier)
        || queue.league_points != 0
        || queue.wins != 0
        || queue.losses != 0
        || queue.provisional_games_remaining != 0
}

fn ranked_text_has_value(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_uppercase().as_str(),
        "" | "NONE" | "NA"
    )
}

fn sgp_ranked_queue_to_entry(
    queue: &SgpRankedQueue,
    fallback_queue_type: &str,
) -> RankedQueueEntry {
    let queue_type = if queue.queue_type.trim().is_empty() {
        fallback_queue_type.to_string()
    } else {
        queue.queue_type.trim().to_string()
    };

    RankedQueueEntry {
        queue_type,
        tier: queue.tier.clone(),
        division: queue.rank.clone(),
        highest_tier: first_non_empty(&[
            &queue.highest_tier,
            &queue.previous_season_achieved_tier,
            &queue.previous_season_highest_tier,
            &queue.previous_season_end_tier,
        ]),
        highest_division: first_non_empty(&[
            &queue.highest_rank,
            &queue.previous_season_achieved_rank,
            &queue.previous_season_highest_rank,
            &queue.previous_season_end_rank,
        ]),
        previous_season_highest_tier: first_non_empty(&[
            &queue.previous_season_highest_tier,
            &queue.previous_season_achieved_tier,
        ]),
        previous_season_highest_division: first_non_empty(&[
            &queue.previous_season_highest_rank,
            &queue.previous_season_achieved_rank,
        ]),
        previous_season_end_tier: queue.previous_season_end_tier.clone(),
        previous_season_end_division: queue.previous_season_end_rank.clone(),
        league_points: queue.league_points,
        wins: queue.wins,
        losses: queue.losses,
        is_provisional: queue.provisional_games_remaining > 0,
    }
}

fn normalize_ranked_queue_type(queue_type: &str) -> String {
    match queue_type.trim().to_ascii_uppercase().as_str() {
        "RANKED_SOLO_5X5" => "RANKED_SOLO_5x5".to_string(),
        other => other.to_string(),
    }
}

fn first_non_empty(values: &[&String]) -> String {
    values
        .iter()
        .find(|value| !value.trim().is_empty())
        .map(|value| (*value).clone())
        .unwrap_or_default()
}

fn has_rank_like_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    [
        "elo",
        "mmr",
        "rating",
        "matchmaking",
        "leaguepoints",
        "tier",
        "rank",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn collect_matching_paths(value: &Value, path: &str, matches: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let next_path = if path.is_empty() {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                if has_rank_like_key(key) {
                    matches.push(next_path.clone());
                }
                collect_matching_paths(child, &next_path, matches);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let next_path = if path.is_empty() {
                    format!("[{index}]")
                } else {
                    format!("{path}[{index}]")
                };
                collect_matching_paths(child, &next_path, matches);
            }
        }
        _ => {}
    }
}

fn collect_rank_like_paths_from_text(text: &str) -> Vec<String> {
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };

    let mut matches = Vec::new();
    collect_matching_paths(&json, "", &mut matches);
    matches.sort();
    matches.dedup();
    matches
}

fn collect_first_participant_challenge_hits(text: &str) -> Vec<String> {
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };

    let hits = json
        .get("json")
        .and_then(|value| value.get("participants"))
        .and_then(Value::as_array)
        .and_then(|participants| participants.first())
        .and_then(|participant| participant.get("challenges"))
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter(|(key, _)| has_rank_like_key(key))
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut hits = hits;
    hits.sort();
    hits.dedup();
    hits
}

fn tencent_sgp_base_url(sgp_server_id: &str) -> AppResult<&'static str> {
    let Some(rso_platform_id) = sgp_server_id.strip_prefix("TENCENT_") else {
        return Err(AppError::SgpUnavailable(
            "当前只支持国服 SGP 深战绩".to_string(),
        ));
    };

    match rso_platform_id {
        "HN1" => Ok("https://hn1-k8s-sgp.lol.qq.com:21019"),
        "HN10" => Ok("https://hn10-k8s-sgp.lol.qq.com:21019"),
        "TJ100" => Ok("https://tj100-sgp.lol.qq.com:21019"),
        "TJ101" => Ok("https://tj101-sgp.lol.qq.com:21019"),
        "NJ100" => Ok("https://nj100-sgp.lol.qq.com:21019"),
        "GZ100" => Ok("https://gz100-sgp.lol.qq.com:21019"),
        "CQ100" => Ok("https://cq100-sgp.lol.qq.com:21019"),
        "BGP2" => Ok("https://bgp2-k8s-sgp.lol.qq.com:21019"),
        "PBE" => Ok("https://pbe-sgp.lol.qq.com:21019"),
        "PREPBE" => Ok("https://prepbe-sgp.lol.qq.com:21019"),
        other => Err(AppError::SgpUnavailable(format!(
            "暂未配置国服大区 {other} 的 SGP 深战绩地址"
        ))),
    }
}

fn should_retry_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

async fn wait_before_retry(attempt: usize) {
    let delay_ms = 500 * (attempt as u64 + 1);
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::client_discovery::discover_primary_client;
    use serde_json::Value;

    fn has_rank_like_key(key: &str) -> bool {
        let normalized = key.to_ascii_lowercase();
        [
            "elo",
            "mmr",
            "rating",
            "matchmaking",
            "leaguepoints",
            "tier",
            "rank",
        ]
        .iter()
        .any(|needle| normalized.contains(needle))
    }

    fn collect_matching_paths(value: &Value, path: &str, matches: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let next_path = if path.is_empty() {
                        key.to_string()
                    } else {
                        format!("{path}.{key}")
                    };
                    if has_rank_like_key(key) {
                        matches.push(next_path.clone());
                    }
                    collect_matching_paths(child, &next_path, matches);
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    collect_matching_paths(child, &format!("{path}[{index}]"), matches);
                }
            }
            _ => {}
        }
    }

    #[tokio::test]
    #[ignore = "手动触发：探测官方对局/段位接口里是否存在 ELO 或近似字段"]
    async fn manual_probe_official_rank_like_fields() {
        let auth = discover_primary_client().expect("应能找到已登录客户端");
        let lcu = LcuClient::new(&auth).expect("应能创建 LCU 客户端");
        let summoner = lcu.current_summoner().await.expect("应能读取当前召唤师");
        let sgp = SgpClient::new(&lcu, &auth, None)
            .await
            .expect("应能创建 SGP 客户端");

        println!(
            "current summoner => '{}' puuid='{}' server='{}'",
            summoner.display_name, summoner.puuid, sgp.sgp_server_id
        );

        let ranked = sgp
            .ranked_stats(&summoner.puuid)
            .await
            .expect("应能读取官方 rankedStats");
        println!("official rankedStats => {ranked:#?}");

        let history = sgp
            .match_history(&summoner.puuid, 0, 1)
            .await
            .expect("应能读取最近对局");
        let game = history.first().expect("最近对局至少应有 1 局");
        let sub_id = sgp
            .sgp_server_id
            .strip_prefix("TENCENT_")
            .unwrap_or(&sgp.sgp_server_id)
            .to_ascii_uppercase();

        let summary_text = sgp
            .http
            .get(format!(
                "{}/match-history-query/v1/products/lol/{}_{}/SUMMARY",
                sgp.base_url, sub_id, game.game_id
            ))
            .header("Authorization", format!("Bearer {}", sgp.access_token))
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .expect("最近对局详情请求应成功")
            .text()
            .await
            .expect("应能读取最近对局详情文本");

        let summary_json: Value =
            serde_json::from_str(&summary_text).expect("对局详情原始 JSON 应能解析");
        let mut all_matches = Vec::new();
        collect_matching_paths(&summary_json, "", &mut all_matches);

        println!("raw summary rank-like keys => {:?}", all_matches);

        let challenge_matches = summary_json
            .get("json")
            .and_then(|value| value.get("participants"))
            .and_then(Value::as_array)
            .and_then(|participants| participants.first())
            .and_then(|participant| participant.get("challenges"))
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter(|(key, _)| has_rank_like_key(key))
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        println!(
            "first participant challenge rank-like keys => {:?}",
            challenge_matches
        );
    }
}
