//! Percy bot internal API client.
//!
//! Proxies dashboard requests to Percy's aiohttp internal API, which owns all
//! guild config mutations and cache invalidation. The client authenticates with
//! a pre-shared bearer token supplied by the caller at construction.
//!
//! Response models live in [`types`].
//!
//! Every endpoint method is a one-liner over three shared helpers: [`PercyClient::send_into`]
//! (deserialize the JSON body), [`PercyClient::send_unit`] (discard the body), and the
//! associated [`PercyClient::check`] (uniform status → [`PercyError`] mapping). `404` maps to
//! [`PercyError::NotFound`]; every other 4xx/5xx maps to [`PercyError::Api`] carrying Percy's
//! response body, so upstream validation errors reach the dashboard.

use reqwest::{Client, RequestBuilder, Response};
use serde::de::DeserializeOwned;

mod types;
pub use types::*;

/// A typed client for Percy's internal API.
#[derive(Clone)]
pub struct PercyClient {
    client: Client,
    base_url: String,
    token: String,
}

impl PercyClient {
    /// Creates a new client from a base URL and bearer token. The base URL has
    /// any trailing slash trimmed so path joins stay canonical. Callers that
    /// hold an optional/partial config should gate construction themselves (see
    /// `PercyConfig::build_client` on the klappstuhl_me side).
    pub fn new(client: Client, base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    // -- Shared request plumbing ---------------------------------------------

    /// Maps Percy's HTTP status into a [`PercyError`], returning the response for
    /// the caller to deserialize on success. `404` → [`PercyError::NotFound`]; any
    /// other 4xx/5xx → [`PercyError::Api`] carrying the response body so Percy's
    /// error text surfaces to the dashboard.
    async fn check(resp: Response) -> Result<Response, PercyError> {
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(PercyError::NotFound);
        }
        if status.is_client_error() || status.is_server_error() {
            return Err(PercyError::Api(resp.text().await.unwrap_or_default()));
        }
        Ok(resp)
    }

    /// Applies auth, sends `req`, and discards the body. For mutations / fire-and-forget.
    async fn send_unit(&self, req: RequestBuilder) -> Result<(), PercyError> {
        Self::check(req.bearer_auth(&self.token).send().await?).await?;
        Ok(())
    }

    /// Applies auth, sends `req`, and deserializes the JSON body into `T`.
    async fn send_into<T: DeserializeOwned>(&self, req: RequestBuilder) -> Result<T, PercyError> {
        let resp = Self::check(req.bearer_auth(&self.token).send().await?).await?;
        Ok(resp.json().await?)
    }

    // -- Guild config --------------------------------------------------------

    /// Fetch full guild config + metadata from Percy.
    pub async fn get_guild(&self, guild_id: u64) -> Result<GuildInfo, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/guilds/{guild_id}"))))
            .await
    }

    /// Patch guild config fields.
    pub async fn patch_guild_config(&self, guild_id: u64, patch: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/config")))
                .json(patch),
        )
        .await
    }

    /// Apply multiple config mutations atomically in one request.
    pub async fn batch_guild_config(
        &self,
        guild_id: u64,
        operations: &[BatchOperation],
    ) -> Result<BatchResponse, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/batch")))
                .json(&serde_json::json!({"operations": operations})),
        )
        .await
    }

    /// Toggle the server-wide AI feature flags. `patch` is `{ "flags": { name: bool, … } }`.
    pub async fn patch_ai_flags(&self, guild_id: u64, patch: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/ai")))
                .json(patch),
        )
        .await
    }

    /// Create or replace a per-channel AI override. An override controlling nothing is removed.
    pub async fn put_ai_override(
        &self,
        guild_id: u64,
        channel_id: u64,
        body: &serde_json::Value,
    ) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .put(self.url(&format!("/api/v1/guilds/{guild_id}/ai/overrides/{channel_id}")))
                .json(body),
        )
        .await
    }

    /// Remove a per-channel AI override.
    pub async fn delete_ai_override(&self, guild_id: u64, channel_id: u64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/ai/overrides/{channel_id}"))),
        )
        .await
    }

    /// Ask the dashboard AI assistant a natural-language question (command palette).
    /// Percy gates this on the guild's `assistant` AI flag + engine health and never errors on
    /// an unavailable assistant — it returns `available: false` with a reason instead.
    pub async fn ai_ask(&self, guild_id: u64, question: &str) -> Result<AiAskResponse, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/ai/ask")))
                .json(&serde_json::json!({ "question": question })),
        )
        .await
    }

    /// Update audit log event flags.
    pub async fn patch_audit_log_flags(
        &self,
        guild_id: u64,
        flags: &std::collections::HashMap<String, bool>,
    ) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/audit-log-flags")))
                .json(flags),
        )
        .await
    }

    /// Fetch guild roles.
    pub async fn get_guild_roles(&self, guild_id: u64) -> Result<Vec<Role>, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/guilds/{guild_id}/roles"))))
            .await
    }

    /// Fetch guild channels.
    pub async fn get_guild_channels(&self, guild_id: u64) -> Result<Vec<Channel>, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/channels"))),
        )
        .await
    }

    /// Fetch guilds the given Discord user can manage (has Manage Server or Admin).
    pub async fn get_user_guilds(&self, discord_user_id: &str) -> Result<Vec<UserGuild>, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/users/{discord_user_id}/guilds"))),
        )
        .await
    }

    /// Fetch a user's *current* avatar URL straight from the bot. Resolved live so
    /// it never goes stale; the dashboard does not persist avatars.
    pub async fn get_user_avatar(&self, discord_user_id: &str) -> Result<UserAvatar, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/users/{discord_user_id}/avatar"))),
        )
        .await
    }

    // -- Members -------------------------------------------------------------

    /// Fetch guild members (paginated, optionally filtered by search term).
    pub async fn get_guild_members(
        &self,
        guild_id: u64,
        limit: u32,
        after: u64,
        search: Option<&str>,
    ) -> Result<MembersResponse, PercyError> {
        let mut query = vec![("limit", limit.to_string()), ("after", after.to_string())];
        if let Some(s) = search {
            if !s.is_empty() {
                query.push(("search", s.to_string()));
            }
        }
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/members")))
                .query(&query),
        )
        .await
    }

    /// Perform a moderation action on a guild member.
    pub async fn member_action(
        &self,
        guild_id: u64,
        user_id: &str,
        action: &str,
        reason: Option<&str>,
        moderator_id: Option<&str>,
    ) -> Result<(), PercyError> {
        let mut body = serde_json::json!({"action": action});
        if let Some(r) = reason {
            body["reason"] = serde_json::Value::String(r.to_string());
        }
        if let Some(m) = moderator_id {
            body["moderator_id"] = serde_json::Value::String(m.to_string());
        }
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/members/{user_id}/action")))
                .json(&body),
        )
        .await
    }

    /// Perform a moderation action on a guild member, forwarding the caller's body verbatim.
    ///
    /// The richer actions (`timeout`, `mute`, `warn`, `softban`, `purge`, …) carry
    /// per-action parameters — `duration`, `delete_message_days`, `limit` — so the dashboard
    /// passes the whole validated body through rather than growing an argument per action.
    /// Percy owns the action whitelist and rejects anything it does not know.
    pub async fn member_action_json(
        &self,
        guild_id: u64,
        user_id: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/members/{user_id}/action")))
                .json(body),
        )
        .await
    }

    /// Fetch a member's recent messages (a live, bounded scan — see [`MemberMessages`]).
    pub async fn get_member_messages(
        &self,
        guild_id: u64,
        user_id: &str,
        limit: u32,
    ) -> Result<MemberMessages, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/members/{user_id}/messages")))
                .query(&[("limit", limit.to_string())]),
        )
        .await
    }

    /// Update a member's roles.
    pub async fn member_roles(
        &self,
        guild_id: u64,
        user_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<(), PercyError> {
        let body = serde_json::json!({"add": add, "remove": remove});
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/members/{user_id}/roles")))
                .json(&body),
        )
        .await
    }

    /// Fetch an aggregated member profile (identity, leveling, moderation history).
    pub async fn get_member_detail(&self, guild_id: u64, user_id: &str) -> Result<MemberDetail, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/members/{user_id}/detail"))),
        )
        .await
    }

    /// Fetch a member's own profile (leveling, economy, command stats — no mod history).
    pub async fn get_member_self(&self, guild_id: u64, user_id: &str) -> Result<MemberSelf, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/members/{user_id}/self"))),
        )
        .await
    }

    pub async fn get_user_settings(&self, discord_id: &str) -> Result<UserSettings, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/users/{discord_id}/settings"))),
        )
        .await
    }

    pub async fn patch_user_settings(&self, discord_id: &str, patch: &UserSettingsPatch) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/users/{discord_id}/settings")))
                .json(patch),
        )
        .await
    }

    /// Fetch a user's avatar history (base64 images + timestamps).
    pub async fn get_member_avatars(
        &self,
        guild_id: u64,
        user_id: &str,
        limit: u32,
    ) -> Result<AvatarHistoryResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/members/{user_id}/avatars")))
                .query(&[("limit", limit.to_string())]),
        )
        .await
    }

    /// Fetch a user's consent-tracked history (names, avatars, presence).
    pub async fn get_user_history(&self, discord_id: &str) -> Result<UserHistory, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/users/{discord_id}/history"))),
        )
        .await
    }

    /// Fetch the full GDPR-style data export for a user (raw JSON passthrough).
    pub async fn export_user_data(&self, discord_id: &str) -> Result<serde_json::Value, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/users/{discord_id}/data-export"))),
        )
        .await
    }

    /// Permanently delete a user's stored presence/avatar/name history.
    pub async fn delete_user_personal_data(&self, discord_id: &str) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/users/{discord_id}/personal-data"))),
        )
        .await
    }

    // -- Sentinel ----------------------------------------------------------

    /// Fetch sentinel configuration for a guild.
    pub async fn get_sentinel(&self, guild_id: u64) -> Result<Option<SentinelInfo>, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/sentinel"))),
        )
        .await
    }

    /// Patch sentinel configuration.
    pub async fn patch_sentinel(&self, guild_id: u64, patch: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/sentinel")))
                .json(patch),
        )
        .await
    }

    /// Send the sentinel verification message to a channel and return the message_id.
    pub async fn send_sentinel_message(
        &self,
        guild_id: u64,
        channel_id: u64,
        title: &str,
        content: &str,
    ) -> Result<u64, PercyError> {
        let body = serde_json::json!({
            "channel_id": channel_id,
            "guild_id": guild_id,
            "title": title,
            "content": content,
        });
        let data: serde_json::Value = self
            .send_into(
                self.client
                    .post(self.url(&format!("/api/v1/guilds/{guild_id}/sentinel/message")))
                    .json(&body),
            )
            .await?;
        Ok(data["message_id"].as_u64().unwrap_or(0))
    }

    /// Enable or disable the sentinel.
    pub async fn toggle_sentinel(&self, guild_id: u64, enabled: bool) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/sentinel/toggle")))
                .json(&serde_json::json!({"enabled": enabled})),
        )
        .await
    }

    // -- Leveling ------------------------------------------------------------

    /// Fetch leveling config for a guild.
    pub async fn get_leveling_config(&self, guild_id: u64) -> Result<LevelingConfig, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/leveling/config"))),
        )
        .await
    }

    /// Fetch the leveling leaderboard.
    pub async fn get_leveling_leaderboard(&self, guild_id: u64, limit: u32) -> Result<LeaderboardResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/leveling/leaderboard")))
                .query(&[("limit", limit.to_string())]),
        )
        .await
    }

    /// Fetch the daily cumulative-XP history for the leveling chart.
    pub async fn get_leveling_xp_history(&self, guild_id: u64, days: u32) -> Result<XpHistoryResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/leveling/xp-history")))
                .query(&[("days", days.to_string())]),
        )
        .await
    }

    /// Update a user's level/xp.
    pub async fn patch_leveling_user(
        &self,
        guild_id: u64,
        user_id: &str,
        patch: &serde_json::Value,
    ) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/leveling/users/{user_id}")))
                .json(patch),
        )
        .await
    }

    /// Patch leveling configuration.
    pub async fn patch_leveling_config(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/leveling/config")))
                .json(body),
        )
        .await
    }

    /// Manage leveling role rewards.
    pub async fn post_leveling_roles(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/leveling/roles")))
                .json(body),
        )
        .await
    }

    /// Create the preset of milestone level-reward roles (levels 5-100).
    pub async fn create_leveling_role_preset(&self, guild_id: u64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/leveling/roles/preset"))),
        )
        .await
    }

    /// Manage leveling XP multipliers.
    pub async fn post_leveling_multipliers(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/leveling/multipliers")))
                .json(body),
        )
        .await
    }

    /// Manage leveling blacklist (channels/roles excluded from XP).
    pub async fn post_leveling_blacklist(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/leveling/blacklist")))
                .json(body),
        )
        .await
    }

    // -- Polls ---------------------------------------------------------------

    /// Fetch polls for a guild.
    pub async fn get_polls(&self, guild_id: u64) -> Result<PollsResponse, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/guilds/{guild_id}/polls"))))
            .await
    }

    /// Create a new poll via a single multipart request: text fields plus an
    /// optional inline banner image. Percy's `POST /polls` endpoint accepts the
    /// image inline and routes it into the guild's shared gallery, so there is no
    /// separate upload step. `options` is sent as one field joined by the ASCII
    /// record separator (`\x1e`), which Percy splits on. Optional text fields are
    /// only sent when non-empty. `image` is `(bytes, mime_type)`. Surfaces Percy's
    /// validation error text via [`PercyError::Api`].
    pub async fn create_poll(
        &self,
        guild_id: u64,
        fields: &serde_json::Value,
        image: Option<(&[u8], &str)>,
    ) -> Result<serde_json::Value, PercyError> {
        fn str_field(v: &serde_json::Value, key: &str) -> String {
            v.get(key).and_then(|x| x.as_str()).unwrap_or("").trim().to_string()
        }

        let options = fields
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|o| o.as_str()).collect::<Vec<_>>().join("\x1e"))
            .unwrap_or_default();
        let duration = fields.get("duration_seconds").and_then(|v| v.as_i64()).unwrap_or(0);

        let mut form = reqwest::multipart::Form::new()
            .text("question", str_field(fields, "question"))
            .text("options", options)
            .text("duration_seconds", duration.to_string());

        // Optional text fields — only forwarded when present and non-empty.
        for key in ["channel_id", "description", "color", "image_url", "thread_question"] {
            let val = str_field(fields, key);
            if !val.is_empty() {
                form = form.text(key, val);
            }
        }

        // Optional inline banner image.
        if let Some((data, mime)) = image {
            let ext = mime.rsplit('/').next().unwrap_or("png");
            let part = reqwest::multipart::Part::bytes(data.to_vec())
                .file_name(format!("banner.{ext}"))
                .mime_str(mime)
                .map_err(|e| PercyError::Api(e.to_string()))?;
            form = form.part("file", part);
        }

        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/polls")))
                .multipart(form),
        )
        .await
    }

    /// Edit a poll.
    pub async fn patch_poll(&self, guild_id: u64, poll_id: i64, patch: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/polls/{poll_id}")))
                .json(patch),
        )
        .await
    }

    /// End a running poll.
    pub async fn end_poll(&self, guild_id: u64, poll_id: i64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/polls/{poll_id}/end"))),
        )
        .await
    }

    // -- Giveaways / Tags ----------------------------------------------------

    /// Fetch giveaways for a guild.
    pub async fn get_giveaways(&self, guild_id: u64) -> Result<GiveawaysResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/giveaways"))),
        )
        .await
    }

    /// Create a giveaway (posts the announcement + schedules the draw). Surfaces
    /// Percy's validation error text via [`PercyError::Api`].
    pub async fn create_giveaway(
        &self,
        guild_id: u64,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/giveaways")))
                .json(body),
        )
        .await
    }

    /// End a giveaway now (draws winners).
    pub async fn end_giveaway(&self, guild_id: u64, giveaway_id: i64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/giveaways/{giveaway_id}/end"))),
        )
        .await
    }

    /// Cancel a giveaway without drawing winners.
    pub async fn delete_giveaway(&self, guild_id: u64, giveaway_id: i64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/giveaways/{giveaway_id}"))),
        )
        .await
    }

    /// Fetch tags for a guild.
    pub async fn get_tags(&self, guild_id: u64) -> Result<TagsResponse, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/guilds/{guild_id}/tags"))))
            .await
    }

    /// Fetch a single tag's full content (for the markdown preview).
    pub async fn get_tag_detail(&self, guild_id: u64, tag_id: i64) -> Result<TagDetail, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/tags/{tag_id}"))),
        )
        .await
    }

    /// Delete a tag (and its aliases).
    pub async fn delete_tag(&self, guild_id: u64, tag_id: i64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/tags/{tag_id}"))),
        )
        .await
    }

    /// Fetch every tag's `(name, content)` for export.
    pub async fn export_tags(&self, guild_id: u64) -> Result<TagExport, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/tags/export"))),
        )
        .await
    }

    /// Bulk-create tags from a parsed `(name, content)` list, owned by `owner_id`.
    pub async fn import_tags(
        &self,
        guild_id: u64,
        tags: &[TagExportRow],
        owner_id: &str,
    ) -> Result<TagImportResult, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/tags/import")))
                .json(&serde_json::json!({"tags": tags, "owner_id": owner_id})),
        )
        .await
    }

    // -- Commands ------------------------------------------------------------

    /// Fetch commands and plonk list for a guild.
    pub async fn get_commands(&self, guild_id: u64) -> Result<CommandsResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/commands"))),
        )
        .await
    }

    /// Toggle a command (enable/disable).
    pub async fn toggle_command(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/commands/toggle")))
                .json(body),
        )
        .await
    }

    /// Manage plonks (add/remove ignored entities).
    pub async fn manage_plonk(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/plonks")))
                .json(body),
        )
        .await
    }

    /// Create, replace, or clear a command's per-guild permission override.
    /// A body with `permissions: null` and no `allowed_roles` removes the override.
    pub async fn set_command_permissions(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/commands/permissions")))
                .json(body),
        )
        .await
    }

    // -- Stats ---------------------------------------------------------------

    /// Fetch guild stats.
    pub async fn get_guild_stats(&self, guild_id: u64) -> Result<GuildStats, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/guilds/{guild_id}/stats"))))
            .await
    }

    /// Fetch bot-wide stats.
    pub async fn get_bot_stats(&self) -> Result<BotStats, PercyError> {
        self.send_into(self.client.get(self.url("/api/v1/bot/stats"))).await
    }

    /// Fetch bot changelog (git log grouped by version tags).
    pub async fn get_changelog(&self) -> Result<ChangelogResponse, PercyError> {
        self.send_into(self.client.get(self.url("/api/v1/bot/changelog"))).await
    }

    /// Composite: fetch guild overview (config summary + stats + bot stats + feature flags)
    /// in a single round-trip.
    pub async fn get_guild_overview(&self, guild_id: u64) -> Result<GuildOverview, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/overview"))),
        )
        .await
    }

    /// Fetch all bot commands (public, no guild context).
    pub async fn get_bot_commands(&self) -> Result<Vec<CommandInfo>, PercyError> {
        let resp: PublicCommandsResponse = self
            .send_into(self.client.get(self.url("/api/v1/commands/public")))
            .await?;
        Ok(resp.commands)
    }

    // -- Autoresponders ------------------------------------------------------

    /// Fetch autoresponders for a guild.
    pub async fn get_autoresponders(&self, guild_id: u64) -> Result<AutorespondersResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/autoresponders"))),
        )
        .await
    }

    /// Create an autoresponder.
    pub async fn create_autoresponder(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/autoresponders")))
                .json(body),
        )
        .await
    }

    /// Delete an autoresponder by trigger.
    pub async fn delete_autoresponder(&self, guild_id: u64, trigger: &str) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/autoresponders/{trigger}"))),
        )
        .await
    }

    /// Patch an autoresponder by trigger.
    pub async fn patch_autoresponder(
        &self,
        guild_id: u64,
        trigger: &str,
        body: &serde_json::Value,
    ) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/autoresponders/{trigger}")))
                .json(body),
        )
        .await
    }

    // -- Economy -------------------------------------------------------------

    /// Fetch economy info (shop items + lottery) for a guild.
    pub async fn get_economy(&self, guild_id: u64) -> Result<EconomyInfo, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/guilds/{guild_id}/economy"))))
            .await
    }

    /// Create a shop item.
    pub async fn create_economy_item(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/economy/items")))
                .json(body),
        )
        .await
    }

    /// Delete a shop item by name.
    pub async fn delete_economy_item(&self, guild_id: u64, name: &str) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/economy/items/{name}"))),
        )
        .await
    }

    /// Fetch economy balances (leaderboard).
    pub async fn get_economy_balances(&self, guild_id: u64, limit: u32) -> Result<BalancesResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/economy/balances")))
                .query(&[("limit", limit.to_string())]),
        )
        .await
    }

    /// Update the guild's economy settings (payout multiplier, rob toggle, daily base, max bet).
    pub async fn patch_economy_settings(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/economy/settings")))
                .json(body),
        )
        .await
    }

    /// Guild-wide game statistics (per-game totals + top players).
    pub async fn get_games_stats(&self, guild_id: u64) -> Result<GamesStats, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/guilds/{guild_id}/games"))))
            .await
    }

    /// Patch a user's economy balance.
    pub async fn patch_economy_balance(
        &self,
        guild_id: u64,
        user_id: &str,
        body: &serde_json::Value,
    ) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/economy/balances/{user_id}")))
                .json(body),
        )
        .await
    }

    /// Create a lottery.
    pub async fn create_lottery(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/economy/lottery")))
                .json(body),
        )
        .await
    }

    /// Delete (cancel) the active lottery.
    pub async fn delete_lottery(&self, guild_id: u64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/economy/lottery"))),
        )
        .await
    }

    // -- Music ---------------------------------------------------------------

    /// Fetch music/equalizer state for a guild.
    pub async fn get_music(&self, guild_id: u64) -> Result<MusicInfo, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/guilds/{guild_id}/music"))))
            .await
    }

    /// Fetch time-synced lyrics for the guild's currently-playing track.
    pub async fn get_music_lyrics(&self, guild_id: u64) -> Result<MusicLyrics, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/music/lyrics"))),
        )
        .await
    }

    /// Apply equalizer settings (preset or custom bands).
    pub async fn post_music_equalizer(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/music/equalizer")))
                .json(body),
        )
        .await
    }

    /// Apply a filter action (nightcore, 8d, lowpass, reset).
    pub async fn post_music_filters(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/music/filters")))
                .json(body),
        )
        .await
    }

    /// Set up the music panel (create channel + send panel message).
    pub async fn post_music_setup(
        &self,
        guild_id: u64,
        body: &serde_json::Value,
    ) -> Result<MusicSetupResponse, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/music/setup")))
                .json(body),
        )
        .await
    }

    /// Reset the music configuration (delete channel, clear config).
    pub async fn post_music_reset(&self, guild_id: u64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/music/reset")))
                .json(&serde_json::json!({})),
        )
        .await
    }

    /// Enable or disable the 24/7 always-on player for a guild.
    pub async fn post_music_247(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/music/247")))
                .json(body),
        )
        .await
    }

    /// Update the DJ mode setting for a guild's music player.
    pub async fn patch_music_dj_mode(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/music/dj-mode")))
                .json(body),
        )
        .await
    }

    /// Control the live player (pause/resume/skip/stop) on behalf of a dashboard
    /// viewer. Percy enforces voice-presence and DJ-mode rules and surfaces a 403
    /// (mapped to [`PercyError::Api`]) when the viewer isn't permitted.
    pub async fn music_control(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/music/control")))
                .json(body),
        )
        .await
    }

    // -- Comics --------------------------------------------------------------

    /// Fetch comic feeds for a guild.
    pub async fn get_comics(&self, guild_id: u64) -> Result<ComicsResponse, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/guilds/{guild_id}/comics"))))
            .await
    }

    /// Create a comic feed.
    pub async fn create_comic(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/comics")))
                .json(body),
        )
        .await
    }

    /// Patch a comic feed by brand.
    pub async fn patch_comic(&self, guild_id: u64, brand: &str, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/comics/{brand}")))
                .json(body),
        )
        .await
    }

    /// Delete a comic feed by brand.
    pub async fn delete_comic(&self, guild_id: u64, brand: &str) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/comics/{brand}"))),
        )
        .await
    }

    /// Trigger a manual push for a comic feed.
    pub async fn push_comic(&self, guild_id: u64, brand: &str) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/comics/{brand}/push"))),
        )
        .await
    }

    // -- Comic catalogue (public) ---------------------------------------------
    //
    // Not guild-scoped: this is the global comic-release catalogue behind
    // percy-dashboard's public comic pages, distinct from the guild-scoped
    // comic *feed* subscriptions above.

    #[allow(clippy::too_many_arguments)]
    fn list_comics_request(&self, query: &ComicBrowseQuery<'_>) -> RequestBuilder {
        let mut req = self.client.get(self.url("/api/v1/comics"));
        for (key, value) in [
            ("brand", query.brand),
            ("series", query.series),
            ("creator", query.creator),
            ("character", query.character),
            ("since", query.since),
            ("until", query.until),
            ("search", query.search),
        ] {
            if let Some(value) = value {
                req = req.query(&[(key, value)]);
            }
        }
        if let Some(page) = query.page {
            req = req.query(&[("page", page.to_string())]);
        }
        if let Some(per_page) = query.per_page {
            req = req.query(&[("per_page", per_page.to_string())]);
        }
        req
    }

    /// List comic releases, filtered and paginated.
    ///
    /// Every filter is optional. `page` is 1-based; Percy caps `per_page` at 100.
    ///
    /// Takes a [`ComicBrowseQuery`] rather than a run of positional arguments: seven of the
    /// filters are `Option<&str>`, so a positional signature lets a caller swap `creator` and
    /// `character` and still compile, silently querying the wrong thing.
    pub async fn list_comics(&self, query: &ComicBrowseQuery<'_>) -> Result<ComicBrowsePage, PercyError> {
        self.send_into(self.list_comics_request(query)).await
    }

    /// Fetch one comic release by brand and slug, with its series neighbours.
    pub async fn get_comic(&self, brand: &str, slug: &str) -> Result<ComicDetail, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/comics/{brand}/{slug}"))))
            .await
    }

    fn list_comic_series_request(&self, brand: Option<&str>) -> RequestBuilder {
        let query: Vec<(&str, String)> = brand.map(|b| vec![("brand", b.to_string())]).unwrap_or_default();
        self.client.get(self.url("/api/v1/comics/series")).query(&query)
    }

    /// List every comic series, optionally scoped to one brand.
    pub async fn list_comic_series(&self, brand: Option<&str>) -> Result<ComicSeriesList, PercyError> {
        self.send_into(self.list_comic_series_request(brand)).await
    }

    fn get_comic_series_request(&self, slug: &str, brand: Option<&str>) -> RequestBuilder {
        let query: Vec<(&str, String)> = brand.map(|b| vec![("brand", b.to_string())]).unwrap_or_default();
        self.client
            .get(self.url(&format!("/api/v1/comics/series/{slug}")))
            .query(&query)
    }

    /// Fetch one series and every issue in it.
    ///
    /// A series slug is derived from a title, so two publishers can collide on one --
    /// pass `brand` whenever the caller knows it, to avoid an ambiguous match.
    pub async fn get_comic_series(&self, slug: &str, brand: Option<&str>) -> Result<ComicSeriesPage, PercyError> {
        self.send_into(self.get_comic_series_request(slug, brand)).await
    }

    fn list_comic_names_request(&self, kind: &str, brand: Option<&str>, limit: Option<u32>) -> RequestBuilder {
        let mut req = self
            .client
            .get(self.url("/api/v1/comics/names"))
            .query(&[("kind", kind)]);
        if let Some(b) = brand {
            req = req.query(&[("brand", b)]);
        }
        if let Some(l) = limit {
            req = req.query(&[("limit", l.to_string())]);
        }
        req
    }

    /// List distinct creator or character names across the catalogue, with appearance counts.
    ///
    /// `kind` is `"creators"` or `"characters"`; anything else is a `400` from Percy.
    pub async fn list_comic_names(
        &self,
        kind: &str,
        brand: Option<&str>,
        limit: Option<u32>,
    ) -> Result<ComicNamesResponse, PercyError> {
        self.send_into(self.list_comic_names_request(kind, brand, limit)).await
    }

    // -- Temp Channels -------------------------------------------------------

    /// Fetch temp channel entries for a guild.
    pub async fn get_temp_channels(&self, guild_id: u64) -> Result<TempChannelsResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/temp-channels"))),
        )
        .await
    }

    /// Create a temp channel config.
    pub async fn create_temp_channel(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/temp-channels")))
                .json(body),
        )
        .await
    }

    /// Patch a temp channel config.
    pub async fn patch_temp_channel(
        &self,
        guild_id: u64,
        channel_id: u64,
        body: &serde_json::Value,
    ) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/temp-channels/{channel_id}")))
                .json(body),
        )
        .await
    }

    /// Delete a temp channel config.
    pub async fn delete_temp_channel(&self, guild_id: u64, channel_id: u64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/temp-channels/{channel_id}"))),
        )
        .await
    }

    // -- Status Feed ---------------------------------------------------------

    /// Fetch status feed info for a guild.
    pub async fn get_status_feed(&self, guild_id: u64) -> Result<StatusFeedInfo, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/status-feed"))),
        )
        .await
    }

    /// Subscribe to or update status feed.
    pub async fn post_status_feed(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/status-feed")))
                .json(body),
        )
        .await
    }

    /// Unsubscribe from status feed.
    pub async fn delete_status_feed(&self, guild_id: u64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/status-feed"))),
        )
        .await
    }

    // -- Lockdowns -----------------------------------------------------------

    /// Fetch locked-down channels for a guild.
    pub async fn get_lockdowns(&self, guild_id: u64) -> Result<LockdownsResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/lockdowns"))),
        )
        .await
    }

    /// Lock channels (apply lockdown overwrites).
    pub async fn lock_channels(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/lockdowns/lock")))
                .json(body),
        )
        .await
    }

    /// Unlock channels.
    pub async fn unlock_channels(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/lockdowns/unlock")))
                .json(body),
        )
        .await
    }

    /// Add or remove a moderation ignored entity (safe automod entity).
    pub async fn manage_moderation_ignore(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/moderation/ignore")))
                .json(body),
        )
        .await
    }

    // -- Highlights ----------------------------------------------------------

    /// Fetch highlights for a guild.
    pub async fn get_highlights(&self, guild_id: u64) -> Result<HighlightsResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/highlights"))),
        )
        .await
    }

    /// Delete a user's highlights.
    pub async fn delete_highlight(&self, guild_id: u64, user_id: &str) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/highlights/{user_id}"))),
        )
        .await
    }

    // -- Emoji Stats ---------------------------------------------------------

    /// Fetch emoji usage stats for a guild.
    pub async fn get_emoji_stats(&self, guild_id: u64, limit: u32) -> Result<EmojiStatsResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/emoji-stats")))
                .query(&[("limit", limit.to_string())]),
        )
        .await
    }

    // -- Audit log (moderation cases) ----------------------------------------

    /// Fetch paginated, filterable moderation cases.
    #[allow(clippy::too_many_arguments)]
    pub async fn get_cases(
        &self,
        guild_id: u64,
        limit: u32,
        offset: u32,
        action: Option<&str>,
        moderator_id: Option<&str>,
        target_id: Option<&str>,
        after: Option<&str>,
        before: Option<&str>,
    ) -> Result<CasesResponse, PercyError> {
        let mut req = self
            .client
            .get(self.url(&format!("/api/v1/guilds/{guild_id}/cases")))
            .query(&[("limit", limit.to_string()), ("offset", offset.to_string())]);
        if let Some(a) = action {
            req = req.query(&[("action", a)]);
        }
        if let Some(m) = moderator_id {
            req = req.query(&[("moderator_id", m)]);
        }
        if let Some(t) = target_id {
            req = req.query(&[("target_id", t)]);
        }
        if let Some(a) = after {
            req = req.query(&[("after", a)]);
        }
        if let Some(b) = before {
            req = req.query(&[("before", b)]);
        }
        self.send_into(req).await
    }

    /// Fetch cases created since a given timestamp (for live notifications).
    pub async fn get_recent_cases(&self, guild_id: u64, since: &str) -> Result<RecentCasesResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/cases/recent")))
                .query(&[("since", since)]),
        )
        .await
    }

    /// Manually open a moderation case (records + announces it; does not perform the action).
    pub async fn create_case(
        &self,
        guild_id: u64,
        action: &str,
        target_id: &str,
        moderator_id: Option<&str>,
        reason: Option<&str>,
    ) -> Result<CaseActionResponse, PercyError> {
        let mut body = serde_json::json!({"action": action, "target_id": target_id});
        if let Some(m) = moderator_id {
            body["moderator_id"] = serde_json::Value::String(m.to_string());
        }
        if let Some(r) = reason {
            body["reason"] = serde_json::Value::String(r.to_string());
        }
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/cases")))
                .json(&body),
        )
        .await
    }

    /// Update a case's reason (also syncs the modlog channel post).
    pub async fn update_case_reason(
        &self,
        guild_id: u64,
        case_index: u64,
        reason: &str,
    ) -> Result<CaseActionResponse, PercyError> {
        self.send_into(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/cases/{case_index}")))
                .json(&serde_json::json!({"reason": reason})),
        )
        .await
    }

    /// Close (delete) a case, removing its modlog channel post.
    pub async fn delete_case(&self, guild_id: u64, case_index: u64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/cases/{case_index}"))),
        )
        .await
    }

    /// Perform a bulk moderation action on multiple members.
    pub async fn bulk_member_action(
        &self,
        guild_id: u64,
        body: &serde_json::Value,
    ) -> Result<BulkActionResponse, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/members/bulk-action")))
                .json(body),
        )
        .await
    }

    /// Fetch per-day activity data for a member (heatmap).
    pub async fn get_member_activity(
        &self,
        guild_id: u64,
        user_id: &str,
        days: u32,
    ) -> Result<ActivityResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/members/{user_id}/activity")))
                .query(&[("days", days.to_string())]),
        )
        .await
    }

    // -- Custom Bot Profile ---------------------------------------------------

    /// Fetch the bot's per-guild profile customization.
    pub async fn get_custom_bot(&self, guild_id: u64) -> Result<CustomBotProfile, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/custom-bot"))),
        )
        .await
    }

    /// Update the bot's per-guild profile customization.
    pub async fn patch_custom_bot(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/custom-bot")))
                .json(body),
        )
        .await
    }

    /// Reset the bot's per-guild profile to defaults.
    pub async fn reset_custom_bot(&self, guild_id: u64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/custom-bot/reset")))
                .json(&serde_json::json!({})),
        )
        .await
    }

    // -- Guild image gallery -------------------------------------------------

    /// List a guild's shared image gallery (newest first; expired images omitted).
    pub async fn get_guild_gallery(&self, guild_id: u64) -> Result<GalleryResponse, PercyError> {
        self.send_into(self.client.get(self.url(&format!("/api/v1/guilds/{guild_id}/gallery"))))
            .await
    }

    /// Upload one or more images into a guild's shared gallery. Each entry is
    /// `(filename, bytes, mime_type)`. Percy forwards them to the klappstuhl.me
    /// guild endpoint under the `files` multipart field.
    pub async fn upload_guild_gallery(
        &self,
        guild_id: u64,
        files: Vec<(String, Vec<u8>, String)>,
    ) -> Result<GalleryUploadResult, PercyError> {
        let mut form = reqwest::multipart::Form::new();
        for (name, bytes, mime) in files {
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(name)
                .mime_str(&mime)
                .map_err(|e| PercyError::Api(e.to_string()))?;
            form = form.part("files", part);
        }

        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/gallery")))
                .multipart(form),
        )
        .await
    }

    /// Delete an image from a guild's shared gallery.
    pub async fn delete_guild_gallery(&self, guild_id: u64, image_id: &str) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/gallery/{image_id}"))),
        )
        .await
    }

    // -- Outbound webhooks (event subscriptions) -----------------------------

    /// The catalogue of events a subscription can listen for.
    pub async fn get_webhook_events(&self, guild_id: u64) -> Result<WebhookEventsResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/webhooks/events"))),
        )
        .await
    }

    /// List every outbound webhook configured for the guild (secrets masked).
    pub async fn get_webhooks(&self, guild_id: u64) -> Result<WebhookSubscriptionsResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/webhooks"))),
        )
        .await
    }

    /// Register a new outbound webhook. The response carries the signing secret
    /// exactly once (masked on every later read) — surface it to the user now.
    pub async fn create_webhook(
        &self,
        guild_id: u64,
        body: &serde_json::Value,
    ) -> Result<WebhookCreateResponse, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/webhooks")))
                .json(body),
        )
        .await
    }

    /// Update a subscription's url, events, label, or enabled state.
    pub async fn patch_webhook(&self, guild_id: u64, sub_id: i64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .patch(self.url(&format!("/api/v1/guilds/{guild_id}/webhooks/{sub_id}")))
                .json(body),
        )
        .await
    }

    /// Delete a subscription and its delivery log.
    pub async fn delete_webhook(&self, guild_id: u64, sub_id: i64) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/webhooks/{sub_id}"))),
        )
        .await
    }

    /// Send a sample `ping` payload to a subscription and report the result synchronously.
    pub async fn test_webhook(&self, guild_id: u64, sub_id: i64) -> Result<WebhookTestResponse, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/webhooks/{sub_id}/test"))),
        )
        .await
    }

    /// Recent delivery attempts for a subscription (its health log).
    pub async fn get_webhook_deliveries(
        &self,
        guild_id: u64,
        sub_id: i64,
        limit: u32,
    ) -> Result<WebhookDeliveriesResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/webhooks/{sub_id}/deliveries")))
                .query(&[("limit", limit.to_string())]),
        )
        .await
    }

    // -- Analytics -----------------------------------------------------------

    /// A zero-filled time-series for one metric (`commands`, `command_failures`,
    /// `xp`, `members`) over a range (`24h`/`7d`/`30d`/`90d`/`1y`), optionally at a
    /// forced granularity (`hour`/`day`/`week`; auto when `None`).
    pub async fn get_analytics_series(
        &self,
        guild_id: u64,
        metric: &str,
        range: &str,
        granularity: Option<&str>,
    ) -> Result<AnalyticsSeries, PercyError> {
        let mut query = vec![("metric", metric.to_string()), ("range", range.to_string())];
        if let Some(g) = granularity {
            query.push(("granularity", g.to_string()));
        }
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/analytics/series")))
                .query(&query),
        )
        .await
    }

    /// Headline numbers for a range, each with a delta vs the preceding window.
    pub async fn get_analytics_summary(&self, guild_id: u64, range: &str) -> Result<AnalyticsSummary, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/analytics/summary")))
                .query(&[("range", range)]),
        )
        .await
    }

    // -- Backup & templates --------------------------------------------------

    /// Export the guild's portable config as a downloadable backup blob (raw JSON passthrough).
    pub async fn export_backup(&self, guild_id: u64) -> Result<serde_json::Value, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/backup/export"))),
        )
        .await
    }

    /// Apply a backup blob to this guild. `dry_run` (default on Percy) previews the
    /// section item counts without writing; pass `false` to actually restore. Returns
    /// Percy's `{ok, dry_run, plan|applied}` report verbatim.
    pub async fn import_backup(
        &self,
        guild_id: u64,
        body: &serde_json::Value,
        dry_run: bool,
    ) -> Result<serde_json::Value, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/backup/import")))
                .query(&[("dry_run", dry_run.to_string())])
                .json(body),
        )
        .await
    }

    /// Public templates (the gallery) plus the ones this guild has published.
    pub async fn get_templates(&self, guild_id: u64) -> Result<TemplatesResponse, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/guilds/{guild_id}/templates"))),
        )
        .await
    }

    /// Publish a template from the current guild's config (or a supplied backup blob).
    /// Surfaces Percy's validation / slug-conflict error text via [`PercyError::Api`].
    pub async fn publish_template(&self, guild_id: u64, body: &serde_json::Value) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/templates")))
                .json(body),
        )
        .await
    }

    /// Apply a template (public or your own) to this guild. `dry_run` previews;
    /// `false` restores. Returns Percy's `{ok, dry_run, plan|applied}` report.
    pub async fn apply_template(
        &self,
        guild_id: u64,
        slug: &str,
        dry_run: bool,
    ) -> Result<serde_json::Value, PercyError> {
        self.send_into(
            self.client
                .post(self.url(&format!("/api/v1/guilds/{guild_id}/templates/{slug}/apply")))
                .query(&[("dry_run", dry_run.to_string())]),
        )
        .await
    }

    /// Delete a template this guild authored.
    pub async fn delete_template(&self, guild_id: u64, slug: &str) -> Result<(), PercyError> {
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/guilds/{guild_id}/templates/{slug}"))),
        )
        .await
    }

    // -- Watchlist -----------------------------------------------------------
    //
    // Not guild-scoped: a watch order is global content and progress is per Discord
    // user. The catalogue endpoints need no session at all, which is what lets the
    // watch-order page render server-side for logged-out visitors.

    /// List every published universe with its title count and total runtime.
    pub async fn list_watch_universes(&self) -> Result<WatchUniversesResponse, PercyError> {
        self.send_into(self.client.get(self.url("/api/v1/watchlist/universes")))
            .await
    }

    /// Fetch one universe's whole page: identity, paths, eras and every title.
    ///
    /// `path` selects which curated route's importance tags are joined in (Percy falls
    /// back to the universe's default); an unknown one is a `404` rather than a silent
    /// fallback. `region` picks the streaming providers; `sort` is `story` or `release`.
    pub async fn get_watch_universe(
        &self,
        slug: &str,
        path: Option<&str>,
        region: &str,
        sort: &str,
    ) -> Result<WatchUniversePage, PercyError> {
        let mut query = vec![("region", region.to_string()), ("sort", sort.to_string())];
        if let Some(path) = path {
            query.push(("path", path.to_string()));
        }
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/watchlist/universes/{slug}")))
                .query(&query),
        )
        .await
    }

    /// Totals for one path: counts, importance breakdown, runtime and hours.
    pub async fn get_watch_universe_stats(&self, slug: &str, path: Option<&str>) -> Result<WatchStats, PercyError> {
        let query: Vec<(&str, String)> = path.map(|p| vec![("path", p.to_string())]).unwrap_or_default();
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/watchlist/universes/{slug}/stats")))
                .query(&query),
        )
        .await
    }

    /// One title in detail, with its credits parent and its neighbours in `sort` order.
    pub async fn get_watch_title(
        &self,
        universe: &str,
        slug: &str,
        region: &str,
        sort: &str,
    ) -> Result<WatchTitleDetail, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/watchlist/titles/{universe}/{slug}")))
                .query(&[("region", region), ("sort", sort)]),
        )
        .await
    }

    /// Fetch a user's watch progress, optionally scoped to one universe.
    pub async fn get_watch_progress(
        &self,
        discord_id: u64,
        universe: Option<&str>,
    ) -> Result<WatchProgressResponse, PercyError> {
        let query: Vec<(&str, String)> = universe.map(|u| vec![("universe", u.to_string())]).unwrap_or_default();
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/watchlist/users/{discord_id}/progress")))
                .query(&query),
        )
        .await
    }

    /// Merge a batch of progress entries and return the merged set.
    ///
    /// `body` is `{ "entries": [{ "title_id": i64, "status": "watched"|"skipped",
    /// "updated_at": ISO-8601 }] }`. Percy resolves conflicts per title by `updated_at`,
    /// so a stale tab cannot overwrite a newer change made on another device.
    pub async fn put_watch_progress(
        &self,
        discord_id: u64,
        body: &serde_json::Value,
    ) -> Result<WatchProgressResponse, PercyError> {
        self.send_into(
            self.client
                .put(self.url(&format!("/api/v1/watchlist/users/{discord_id}/progress")))
                .json(body),
        )
        .await
    }

    /// Reset a user's progress, optionally for one universe only.
    pub async fn delete_watch_progress(&self, discord_id: u64, universe: Option<&str>) -> Result<(), PercyError> {
        let query: Vec<(&str, String)> = universe.map(|u| vec![("universe", u.to_string())]).unwrap_or_default();
        self.send_unit(
            self.client
                .delete(self.url(&format!("/api/v1/watchlist/users/{discord_id}/progress")))
                .query(&query),
        )
        .await
    }

    /// Fetch a user's watchlist preferences (Percy returns defaults if they saved none).
    pub async fn get_watch_prefs(&self, discord_id: u64) -> Result<WatchPrefs, PercyError> {
        self.send_into(
            self.client
                .get(self.url(&format!("/api/v1/watchlist/users/{discord_id}/prefs"))),
        )
        .await
    }

    /// Update the given preference fields, leaving the rest untouched.
    pub async fn patch_watch_prefs(&self, discord_id: u64, body: &serde_json::Value) -> Result<WatchPrefs, PercyError> {
        self.send_into(
            self.client
                .patch(self.url(&format!("/api/v1/watchlist/users/{discord_id}/prefs")))
                .json(body),
        )
        .await
    }
}

// -- Error type --------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PercyError {
    #[error("guild not found")]
    NotFound,
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Api(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> PercyClient {
        PercyClient::new(Client::new(), "http://localhost", "token")
    }

    fn query_pairs(req: RequestBuilder) -> Vec<(String, String)> {
        req.build()
            .unwrap()
            .url()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    #[test]
    fn list_comics_omits_unset_filters() {
        let pairs = query_pairs(client().list_comics_request(&ComicBrowseQuery {
            brand: Some("marvel"),
            search: Some("spider"),
            page: Some(2),
            ..Default::default()
        }));
        assert!(pairs.contains(&("brand".to_string(), "marvel".to_string())));
        assert!(pairs.contains(&("search".to_string(), "spider".to_string())));
        assert!(pairs.contains(&("page".to_string(), "2".to_string())));
        assert!(!pairs.iter().any(|(k, _)| k == "series"));
        assert!(!pairs.iter().any(|(k, _)| k == "creator"));
        assert!(!pairs.iter().any(|(k, _)| k == "character"));
        assert!(!pairs.iter().any(|(k, _)| k == "since"));
        assert!(!pairs.iter().any(|(k, _)| k == "until"));
        assert!(!pairs.iter().any(|(k, _)| k == "per_page"));
    }

    #[test]
    fn list_comics_keeps_creator_and_character_distinct() {
        let pairs = query_pairs(client().list_comics_request(&ComicBrowseQuery {
            creator: Some("jane-doe"),
            character: Some("spider-man"),
            ..Default::default()
        }));
        assert!(pairs.contains(&("creator".to_string(), "jane-doe".to_string())));
        assert!(pairs.contains(&("character".to_string(), "spider-man".to_string())));
    }

    #[test]
    fn list_comics_with_no_filters_has_no_query() {
        let pairs = query_pairs(client().list_comics_request(&ComicBrowseQuery::default()));
        assert!(pairs.is_empty());
    }

    #[test]
    fn list_comic_series_brand_is_optional() {
        assert!(query_pairs(client().list_comic_series_request(None)).is_empty());
        assert_eq!(
            query_pairs(client().list_comic_series_request(Some("dc"))),
            vec![("brand".to_string(), "dc".to_string())]
        );
    }

    #[test]
    fn get_comic_series_brand_is_optional() {
        assert!(query_pairs(client().get_comic_series_request("spider-man", None)).is_empty());
        assert_eq!(
            query_pairs(client().get_comic_series_request("spider-man", Some("marvel"))),
            vec![("brand".to_string(), "marvel".to_string())]
        );
    }

    #[test]
    fn list_comic_names_always_sends_kind_and_optionally_brand_and_limit() {
        let pairs = query_pairs(client().list_comic_names_request("creators", None, None));
        assert_eq!(pairs, vec![("kind".to_string(), "creators".to_string())]);

        let pairs = query_pairs(client().list_comic_names_request("characters", Some("marvel"), Some(25)));
        assert!(pairs.contains(&("kind".to_string(), "characters".to_string())));
        assert!(pairs.contains(&("brand".to_string(), "marvel".to_string())));
        assert!(pairs.contains(&("limit".to_string(), "25".to_string())));
    }
}
