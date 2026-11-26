use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;

use super::error::{AppError, AppResult};
use super::lcu::LcuClient;
use super::models::{
    ClientAuth, Game, GameTeam, IdentityPlayer,
    OfficialApiDebugResponse, OfficialApiProbeSample, Participant, ParticipantIdentity,
    ParticipantStats, ParticipantTimeline, RankedQueueEntry, RankedQueueMap,
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
    /// 留一份 LCU 客户端：长爬取中途 token 会过期，遇到 401 时用它重拉一次，
    /// 免得整段爬取因为一个过期 token 提前失去命中率。
    lcu: LcuClient,
    tokens: std::sync::RwLock<SgpTokens>,
    sgp_server_id: String,
}

/// SGP 请求用到的两个 token。
///
/// LCU 重启或长时间运行后都会失效，所以不存成裸字段，
/// 而是放进锁里支持原地刷新。读多写极少（只在 401 时写），
/// 用 `RwLock` 足够，且临界区里没有 `.await`。
struct SgpTokens {
    access_token: String,
    league_session_token: String,
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
        // 显式开压缩。reqwest 在编译期启用对应 feature 后，
        // `Accepts::default()` 本来就会把 gzip/brotli/deflate 设为 true，
        // 这里写出来是为了**把这个依赖摆在明面上**：
        // SGP 的 SUMMARY 一页 50 局未压缩 6.58 MB，gzip 后 0.44 MB。
        // 一旦有人误删 Cargo.toml 里的压缩 feature，这几个方法会直接
        // 编译失败，而不是悄悄退化成全量下载（那正是之前的故障形态：
        // 并发开到 32 也只有 2 req/s，因为瓶颈在带宽而不在请求数）。
        let http = Client::builder()
            .danger_accept_invalid_certs(true)
            .no_proxy()
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .timeout(std::time::Duration::from_secs(14))
            .build()
            .map_err(AppError::Http)?;

        Ok(Self {
            http,
            base_url: base_url.to_string(),
            lcu: lcu.clone(),
            tokens: std::sync::RwLock::new(SgpTokens {
                access_token,
                league_session_token,
            }),
            sgp_server_id,
        })
    }

    /// 当前 entitlements access token。
    fn access_token(&self) -> String {
        self.tokens
            .read()
            .map(|tokens| tokens.access_token.clone())
            .unwrap_or_default()
    }

    /// 当前 league-session token。
    fn league_session_token(&self) -> String {
        self.tokens
            .read()
            .map(|tokens| tokens.league_session_token.clone())
            .unwrap_or_default()
    }

    /// 重新从 LCU 拉取 token 并原地替换。
    ///
    /// ## 为什么需要它
    ///
    /// 爬取一旦跑起来就是几分钟起步，中途 LCU 的 entitlements /
    /// league-session token 会过期，表现为**突发的 401**。
    /// 从计数器上完全看不出是过期 —— 和普通「请求失败」长得一模一样
    /// （实测一次 86 个失败账号里就混着若干 401）。
    /// 刷新一次即可自愈，不必把已经展开的图谱整个扔掉重来。
    pub async fn refresh_tokens(&self) -> AppResult<()> {
        let access_token = self.lcu.entitlements_access_token().await?;
        let league_session_token = self
            .lcu
            .league_session_token()
            .await
            .unwrap_or_else(|_| access_token.clone());
        if let Ok(mut tokens) = self.tokens.write() {
            tokens.access_token = access_token;
            tokens.league_session_token = league_session_token;
        }
        Ok(())
    }

    /// 带「401 自愈」的认证 GET。
    ///
    /// 先用当前 token 发一次；若返回 401（token 过期），刷新后原样重发一次。
    /// 只自愈一次：真要是凭证彻底无效，刷新也救不回来，
    /// 反复打 LCU 没有意义，还不如把错误如实抛给上层。
    async fn get_authed(
        &self,
        url: String,
        query: &[(&str, String)],
        use_league_session: bool,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let send = |token: String| {
            self.http
                .get(&url)
                .header("Authorization", format!("Bearer {token}"))
                .header("User-Agent", USER_AGENT)
                .query(query)
                .send()
        };

        let pick = |client: &Self| {
            if use_league_session {
                client.league_session_token()
            } else {
                client.access_token()
            }
        };

        let response = send(pick(self)).await?;
        if response.status() == StatusCode::UNAUTHORIZED && self.refresh_tokens().await.is_ok() {
            return send(pick(self)).await;
        }
        Ok(response)
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
                format!("Bearer {}", self.league_session_token()),
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
                format!("Bearer {}", self.league_session_token()),
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
                .get_authed(
                    format!(
                        "{}/match-history-query/v1/products/lol/player/{}/SUMMARY",
                        self.base_url, puuid
                    ),
                    &[
                        ("startIndex", start_index.to_string()),
                        ("count", count.to_string()),
                    ],
                    false,
                )
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
            .get_authed(
                format!(
                    "{}/match-history-query/v1/products/lol/{}_{}/SUMMARY",
                    self.base_url, sub_id, game_id
                ),
                &[],
                false,
            )
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
            format!("Bearer {}", sgp.league_session_token()),
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
        .header("Authorization", format!("Bearer {}", sgp.access_token()))
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
            .header("Authorization", format!("Bearer {}", sgp.access_token()))
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

/// SGP 单局里每个玩家携带的挑战数据（`challenges`）。
///
/// **只声明我们真正读的两个键。** SGP 实测每局每人会下发约 120 个挑战项，
/// 序列化后 3.7 KB/人、37 KB/局，占整局下载体积的 26%。
/// 之前用 `serde_json::Value` 接，等于把 120 个键全部反序列化成 map，
/// 解析完又只读其中 2 个键，剩下的立刻被丢掉 —— 纯浪费 CPU 和内存。
///
/// serde 对结构体默认「未知字段跳过」，所以只要不声明那些键，
/// 它们就不会被分配。这是单点收益最大的一处解析优化。
///
/// **注意类型必须是 f64 而不是 u32**：SGP 实际下发的是
/// `31.0` 这样的浮点值，用 u32 接会直接解析失败
/// （实机验证时踩到过：`invalid type: floating point 31.0, expected u32`）。
/// 老实现走 `json_value_to_u32` 做了宽松转换，这里必须保留同样的宽容度。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SgpChallenges {
    #[serde(default)]
    enemy_champion_immobilizations: f64,
    #[serde(default)]
    immobilize_and_kill_with_ally: f64,
}

/// 把挑战项里的浮点计数收敛成内部使用的 u32（负数与 NaN 归零）。
fn challenge_count(value: f64) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    value.round().min(u32::MAX as f64) as u32
}

/// 宽松地把 JSON 数字解析成 `u32`（负数 / 越界 / 浮点一律收敛，绝不报错）。
///
/// ## 为什么必须有这个
///
/// SGP 会在数值字段里下发 `-2147483648`（`i32::MIN`）这类哨兵值。
/// 只要有一个字段声明成 `u32`，serde 就会**直接让整个响应解析失败** ——
/// 而失败的单位不是「一个字段」也不是「一局」，是**整个账号的一整页**：
/// 一个玩家身上出现一次异常值，该账号最多 50 局全部丢弃。
///
/// 实测代价：801 个账号里 **86 个**因此失败（约 11%），
/// 且全部被记成「请求失败」，从计数器上完全看不出是解析问题。
///
/// 这已经是同一类 bug 第二次出现（上一次是 `challenges` 用 `u32` 接浮点
/// `31.0`）。教训：**来自外部的数值字段，永远不要假设它落在类型范围内**，
/// 能用宽松解析就别让它抛错。
///
/// 用 Visitor 而不是先转 `serde_json::Value`：这是热路径
/// （每页 50 局 × 10 人 × 38 个字段 ≈ 19000 次调用），
/// 走 Value 会多一次分配。
fn deserialize_u32_lenient<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct LenientU32;

    impl<'de> serde::de::Visitor<'de> for LenientU32 {
        type Value = u32;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("整数或浮点数（负数与越界值一律归零）")
        }

        fn visit_u64<E>(self, value: u64) -> Result<u32, E> {
            Ok(value.min(u32::MAX as u64) as u32)
        }

        fn visit_i64<E>(self, value: i64) -> Result<u32, E> {
            Ok(value.clamp(0, u32::MAX as i64) as u32)
        }

        fn visit_f64<E>(self, value: f64) -> Result<u32, E> {
            if !value.is_finite() || value <= 0.0 {
                return Ok(0);
            }
            Ok(value.round().min(u32::MAX as f64) as u32)
        }
    }

    deserializer.deserialize_any(LenientU32)
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
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    participant_id: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    champion_id: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    spell1_id: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    spell2_id: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    team_id: u32,
    #[serde(default)]
    win: bool,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    kills: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    deaths: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    assists: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    turret_kills: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    gold_earned: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    total_minions_killed: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    neutral_minions_killed: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    total_damage_dealt_to_champions: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    damage_self_mitigated: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    total_damage_taken: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    total_heal: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    vision_score: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    champ_level: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    player_subteam_id: u32,
    /// 我们只从中读 `enemyChampionImmobilizations` 和
    /// `immobilizeAndKillWithAlly` 两个键，但**不能**用 `Value` 接：
    /// SGP 每局每个玩家会带回来约 120 个挑战项（3.7 KB），
    /// 10 人就是 37 KB —— 占单局下载体积的 26%，而其中 99% 是垃圾。
    ///
    /// 用 `ReferenceGameChallenges` 只声明这两个键，serde 会把其余的全部
    /// 跳过（不分配、不建 map）。实测能把解析后驻留内存砍掉一大半。
    #[serde(default)]
    challenges: SgpChallenges,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    item0: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    item1: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    item2: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    item3: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    item4: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    item5: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    item6: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    perk0: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    perk1: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    perk2: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    perk3: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    perk4: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    perk5: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    player_augment1: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    player_augment2: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    player_augment3: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    player_augment4: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    player_augment5: u32,
    #[serde(default, deserialize_with = "deserialize_u32_lenient")]
    player_augment6: u32,
    #[serde(default)]
    lane: String,
    #[serde(default)]
    role: String,
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
            // 这两个数直接从已裁剪的 `SgpChallenges` 取，不再走 Value 查找。
            // API 下发的是浮点，统一收敛成 u32。
            let enemy_champion_immobilizations =
                challenge_count(participant.challenges.enemy_champion_immobilizations);
            let immobilize_and_kill_with_ally =
                challenge_count(participant.challenges.immobilize_and_kill_with_ally);

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

    /// 回归测试：SGP 单局解析必须**跳过** missions、只取 challenges 里那两个键。
    ///
    /// 背景：SGP 每局每人会带约 120 个挑战项 + 一大堆 `missions`，
    /// 两者合计占单局体积的 **69.5%**（实测：missions 43.2% + challenges 26.3%），
    /// 而我们只读其中 2 个数。
    ///
    /// 之前 `challenges` 用 `serde_json::Value` 接，会把 120 个键全部
    /// 反序列化成 map 再丢掉，纯浪费 CPU/内存。这条测试锁住
    /// 「只声明需要的键」这个约束：谁把它改回 `Value`，
    /// 或误删 `SgpChallenges`，测试立刻失败。
    #[test]
    fn participant_parsing_ignores_missions_and_trims_challenges() {
        let payload = serde_json::json!({
            "gameId": 8969791979u64,
            "gameCreation": 1773928417643i64,
            "gameDuration": 1320,
            "gameMode": "KIWI",
            "gameType": "MATCHED_GAME",
            "queueId": 2400,
            "teams": [{"teamId": 100, "objectives": {"tower": {"kills": 3}}, "bans": []}],
            "participants": [{
                "participantId": 1,
                "puuid": "p-1",
                "riotIdGameName": "测试玩家",
                "riotIdTagline": "1234",
                "championId": 64,
                "teamId": 100,
                "win": true,
                "kills": 12,
                "deaths": 3,
                "assists": 20,
                "totalDamageDealtToChampions": 45678,
                "playerAugment1": 101,
                "playerAugment2": 202,
                "lane": "MIDDLE",
                "role": "SOLO",
                // 我们不读的字段，解析时应被完整跳过。
                "missions": {
                    "Missions_ChampionsKilled": 12,
                    "Missions_CreepScore": 210,
                    "PlayerScore0": 1
                },
                "challenges": {
                    // ★ 真实 API 下发的是浮点，必须用 f64 接（实机验证过）。
                    "enemyChampionImmobilizations": 7.0,
                    "immobilizeAndKillWithAlly": 3.0,
                    // 一大堆与目标键同前缀/同类型的干扰项。
                    "12AssistStreakCount": 1,
                    "SWARM_DefeatAatrox": 0,
                    "kda": 10.67,
                    "immobilizeAndKillWithAllyRatio": 0.5,
                    "enemyChampionImmobilizationsTotal": 99
                }
            }]
        });

        let game: SgpGameJson = serde_json::from_value(payload).expect("SGP 单局应能解析");
        assert_eq!(game.game_id, 8969791979);
        assert_eq!(game.queue_id, 2400);
        assert_eq!(game.participants.len(), 1);

        let p = &game.participants[0];
        // 关键断言：两个挑战项被正确读出，干扰项不串味。
        assert_eq!(p.challenges.enemy_champion_immobilizations, 7.0);
        assert_eq!(p.challenges.immobilize_and_kill_with_ally, 3.0);

        // 转成内部 Game 后，值仍应保留。
        let game = sgp_game_json_to_lcu_game(game);
        assert_eq!(game.participants[0].stats.enemy_champion_immobilizations, 7);
        assert_eq!(game.participants[0].stats.immobilize_and_kill_with_ally, 3);
        assert_eq!(game.participants[0].stats.kills, 12);
        assert_eq!(game.participants[0].stats.player_augment1, 101);
        // 参与者身份应从 participants 合成（SGP 没有 participantIdentities）。
        assert_eq!(game.participant_identities[0].player.puuid, "p-1");
        assert_eq!(game.participant_identities[0].player.game_name, "测试玩家");
        assert_eq!(game.teams[0].tower_kills, 3);
    }

    /// 回归测试：挑战项缺失时不应报错（默认 0）。
    #[test]
    fn participant_parsing_tolerates_missing_challenges() {
        let payload = serde_json::json!({
            "gameId": 1,
            "queueId": 2400,
            "participants": [{"participantId": 1, "puuid": "x"}]
        });
        let game: SgpGameJson = serde_json::from_value(payload).expect("缺 challenges 也应能解析");
        assert_eq!(game.participants[0].challenges.enemy_champion_immobilizations, 0.0);
        assert_eq!(game.participants[0].challenges.immobilize_and_kill_with_ally, 0.0);
    }

    /// 回归测试：挑战项是浮点，必须能宽容收敛成 u32。
    ///
    /// 这是实机跑出来的 bug：真实 SGP 下发 `31.0`，
    /// 一开始用 `u32` 接，直接
    /// `invalid type: floating point '31.0', expected u32` 解析失败，
    /// 整个对局列表都拉不回来。老实现靠 `json_value_to_u32` 兜底，
    /// 用结构体接之后必须显式补上这层转换。
    #[test]
    fn challenge_count_coerces_floats_leniently() {
        assert_eq!(challenge_count(31.0), 31);
        assert_eq!(challenge_count(7.4), 7);
        assert_eq!(challenge_count(7.6), 8);
        assert_eq!(challenge_count(0.0), 0);
        assert_eq!(challenge_count(-3.0), 0);
        assert_eq!(challenge_count(f64::NAN), 0);
        assert_eq!(challenge_count(f64::INFINITY), 0);
        // 溢出保护：不应 panic，截到 u32 上限。
        assert_eq!(challenge_count(1e30), u32::MAX);

        // 端到端：真实形态的浮点 payload 必须能解析成功。
        let payload = serde_json::json!({
            "gameId": 2,
            "queueId": 2400,
            "participants": [{
                "participantId": 1,
                "puuid": "x",
                "challenges": {
                    "enemyChampionImmobilizations": 31.0,
                    "immobilizeAndKillWithAlly": 4.0
                }
            }]
        });
        let game: SgpGameJson = serde_json::from_value(payload).expect("浮点挑战项应能解析");
        let game = sgp_game_json_to_lcu_game(game);
        assert_eq!(game.participants[0].stats.enemy_champion_immobilizations, 31);
        assert_eq!(game.participants[0].stats.immobilize_and_kill_with_ally, 4);
    }

    /// 回归测试：数值字段收到负数哨兵值时，**不能**让整页对局解析失败。
    ///
    /// 这是实机跑出来的数据丢失 bug：SGP 在 `damageSelfMitigated` 里
    /// 下发了 `-2147483648`（`i32::MIN`）。该字段原本声明成 `u32`，
    /// serde 立刻抛
    /// `invalid value: integer '-2147483648', expected u32`，
    /// 而失败的单位是**整个账号的一整页（最多 50 局）**，
    /// 801 个账号里有 86 个因此被白白丢弃 —— 且从计数上只显示「请求失败」，
    /// 完全看不出是解析问题。
    ///
    /// 现在所有外部数值字段都走 `deserialize_u32_lenient`，
    /// 负数/越界/浮点一律收敛，绝不抛错。这条测试锁住这个行为。
    #[test]
    fn participant_numeric_fields_tolerate_negative_sentinels() {
        // 真实形态：一个字段是负数哨兵值，另一个字段正常。
        let payload = serde_json::json!({
            "gameId": 3,
            "queueId": 2400,
            "participants": [{
                "participantId": 1,
                "puuid": "x",
                "damageSelfMitigated": -2147483648_i64,   // i32::MIN 哨兵
                "totalDamageDealtToChampions": 100000,
                "goldEarned": -1,                          // 异常负值
                "totalMinionsKilled": 123.6,               // 浮点
                "visionScore": 4294967296_u64              // 超 u32 上限
            }]
        });

        // 关键：整页必须能解析成功，而不是抛错丢掉全部对局。
        let game: SgpGameJson =
            serde_json::from_value(payload).expect("负数哨兵值不应导致整页解析失败");
        let p = &game.participants[0];

        // 异常值收敛为 0，正常值原样保留，浮点四舍五入，越界截断。
        assert_eq!(p.damage_self_mitigated, 0);
        assert_eq!(p.gold_earned, 0);
        assert_eq!(p.total_damage_dealt_to_champions, 100000);
        assert_eq!(p.total_minions_killed, 124);
        assert_eq!(p.vision_score, u32::MAX);
    }

    /// 端到端：带负数哨兵值的整局仍应转成内部 `Game`（不丢参与者）。
    #[test]
    fn negative_sentinel_does_not_drop_participants() {
        let payload = serde_json::json!({
            "gameId": 4,
            "queueId": 2400,
            "participants": [{
                "participantId": 1,
                "puuid": "p-neg",
                "riotIdGameName": "负值玩家",
                "damageSelfMitigated": -2147483648_i64,
                "championId": 101
            }]
        });
        let game: SgpGameJson = serde_json::from_value(payload).expect("应能解析");
        let game = sgp_game_json_to_lcu_game(game);
        assert_eq!(game.participants.len(), 1, "参与者不应被丢弃");
        assert_eq!(game.participants[0].stats.damage_self_mitigated, 0);
        assert_eq!(game.participants[0].champion_id, 101);
    }

    /// 实机验证：用真实 `SgpClient` 拉一页，确认压缩确实生效且能解析。
    ///
    /// 这条测试需要本机已启动并登录国服客户端，所以默认 `#[ignore]`。
    /// 用它来复核「查半天只回来两个对局」是否真的被治好：
    /// ```text
    /// cargo test --lib manual_verify_sgp_compressed_fetch -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "手动触发：需要本机已登录的国服客户端"]
    async fn manual_verify_sgp_compressed_fetch() {
        use std::time::Instant;

        let auth = discover_primary_client().expect("应能找到已登录客户端");
        let lcu = LcuClient::new(&auth).expect("应能创建 LCU 客户端");
        let summoner = lcu.current_summoner().await.expect("应能读取当前召唤师");
        let sgp = SgpClient::new(&lcu, &auth, None)
            .await
            .expect("应能创建 SGP 客户端");

        let puuid = summoner.puuid.clone();
        let started = Instant::now();
        let games = sgp
            .match_history(&puuid, 0, 50)
            .await
            .expect("应能拉到对局列表");
        let elapsed = started.elapsed();

        println!(
            "compressed fetch => {} 局 / {:?} / 平均 {:.1} ms/局",
            games.len(),
            elapsed,
            elapsed.as_millis() as f64 / games.len().max(1) as f64
        );

        assert!(!games.is_empty(), "应至少拉到一局");
        // 解析出来的对局必须带着参与者身份（爬取引擎靠它扩散）。
        let with_players = games
            .iter()
            .filter(|game| !game.participant_identities.is_empty())
            .count();
        println!("其中带参与者身份的对局: {with_players}/{}", games.len());
        assert!(with_players > 0, "对局应带出参与者身份，否则图谱无法扩散");
    }

    /// 回归测试：SGP HTTP 客户端必须开启压缩。
    ///
    /// 实测 SGP 一页 50 局未压缩 **6.58 MB**，gzip 后仅 **0.44 MB**，
    /// 耗时从 ~2200 ms 降到 ~595 ms。此前 `reqwest` 只开了
    /// `json` + `rustls-tls`，**根本不发 Accept-Encoding**，
    /// 每页实打实下载 6.5 MB —— 这就是「查半天只回来两个对局」的真凶
    /// （并发加到 32 也只有 2 req/s，瓶颈是带宽不是请求数）。
    ///
    /// 这条测试锁住 Cargo.toml 里的 gzip/brotli/deflate feature。
    #[test]
    fn reqwest_client_compression_features_enabled() {
        // 能调用 .gzip()/.brotli()/.deflate() 本身就证明 feature 已开启
        // （feature 缺失时这些方法不存在，编译直接失败）。
        let client = Client::builder()
            .danger_accept_invalid_certs(true)
            .no_proxy()
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .build()
            .expect("应能创建启用压缩的 HTTP client");
        drop(client);
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
            .header("Authorization", format!("Bearer {}", sgp.access_token()))
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
