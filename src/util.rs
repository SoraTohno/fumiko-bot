use crate::types;
use crate::types::{Context as PoiseContext, QueryMode};
use poise::serenity_prelude::{self as serenity, CreateEmbedAuthor};
use regex::Regex;
use sqlx::postgres::PgPool;
use sqlx::types::chrono::{DateTime, Utc};
use std::sync::OnceLock;

pub async fn ensure_user_exists(pool: &PgPool, user: &serenity::User) -> Result<(), types::Error> {
    sqlx::query!(
        "INSERT INTO discord_users (user_id, username) 
         VALUES ($1, $2) 
         ON CONFLICT (user_id) 
         DO UPDATE SET username = $2, updated_at = CURRENT_TIMESTAMP",
        user.id.get() as i64,
        user.name
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn ensure_server_exists(
    pool: &PgPool,
    guild_id: serenity::GuildId,
    guild_name: &str,
) -> Result<(), types::Error> {
    sqlx::query!(
        "INSERT INTO discord_servers (server_id, server_name) 
         VALUES ($1, $2) 
         ON CONFLICT (server_id) 
         DO UPDATE SET server_name = $2, updated_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64,
        guild_name
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub fn normalize_isbn(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

pub fn is_valid_isbn10(s: &str) -> bool {
    if s.len() != 10 {
        return false;
    }
    let mut sum = 0u32;
    for (i, ch) in s.chars().enumerate() {
        let val = if i == 9 && ch == 'X' {
            10
        } else if ch.is_ascii_digit() {
            ch.to_digit(10).unwrap()
        } else {
            return false;
        };
        sum += (10 - i as u32) * val;
    }
    sum % 11 == 0
}

pub fn is_valid_isbn13(s: &str) -> bool {
    if s.len() != 13 || !s.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let digits: Vec<u32> = s.chars().map(|c| c.to_digit(10).unwrap()).collect();
    let mut sum = 0u32;
    for i in 0..12 {
        sum += digits[i] * if i % 2 == 0 { 1 } else { 3 };
    }
    let check = (10 - (sum % 10)) % 10;
    check == digits[12]
}

pub fn detect_query_mode(query: &str) -> QueryMode {
    let n = normalize_isbn(query);
    if is_valid_isbn13(&n) || is_valid_isbn10(&n) {
        QueryMode::Isbn
    } else {
        QueryMode::Title
    }
}

pub async fn get_guild_name(ctx: &PoiseContext<'_>) -> String {
    if let Some(guild) = ctx.guild() {
        return guild.name.clone();
    }

    if let Some(guild_id) = ctx.guild_id() {
        match guild_id
            .to_partial_guild(ctx.serenity_context().http.clone())
            .await
        {
            Ok(partial) => partial.name,
            Err(err) => {
                if let serenity::Error::Http(http_err) = &err {
                    if http_err.status_code() == Some(serenity::http::StatusCode::FORBIDDEN) {
                        return format!("Server {}", guild_id.get());
                    }
                }

                log_error_with_source("Failed to fetch guild name", &err);
                format!("Server {}", guild_id.get())
            }
        }
    } else {
        "Direct Message".to_string()
    }
}

pub async fn get_guild_icon_url(ctx: &PoiseContext<'_>) -> Option<String> {
    if let Some(guild) = ctx.guild() {
        return guild.icon_url();
    }

    if let Some(guild_id) = ctx.guild_id() {
        match guild_id
            .to_partial_guild(ctx.serenity_context().http.clone())
            .await
        {
            Ok(partial) => partial.icon_url(),
            Err(err) => {
                if let serenity::Error::Http(http_err) = &err {
                    if http_err.status_code() == Some(serenity::http::StatusCode::FORBIDDEN) {
                        return None;
                    }
                }

                log_error_with_source("Failed to fetch guild icon", &err);
                None
            }
        }
    } else {
        None
    }
}

fn mention_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"<[@#&][^>]+>").expect("valid mention regex"))
}

fn id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\b\d{10,}\b").expect("valid id regex"))
}

fn user_tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"@[A-Za-z0-9_]{2,32}").expect("valid user tag regex"))
}

pub fn anonymize_log_message(message: &str) -> String {
    let without_mentions = mention_regex().replace_all(message, "[redacted]");
    let without_ids = id_regex().replace_all(&without_mentions, "[id]");
    user_tag_regex()
        .replace_all(&without_ids, "@redacted")
        .to_string()
}

pub fn log_error(message: impl AsRef<str>) {
    eprintln!("{}", anonymize_log_message(message.as_ref()));
}

pub fn log_error_with_source(message: &str, err: &impl std::fmt::Display) {
    log_error(format!("{message}: {err}"));
}

pub fn log_cache_stat(message: impl AsRef<str>) {
    println!("{}", anonymize_log_message(message.as_ref()));
}

pub fn embed_author_with_icon(
    label: impl Into<String>,
    icon_url: Option<String>,
) -> CreateEmbedAuthor {
    let mut author = CreateEmbedAuthor::new(label);
    if let Some(url) = icon_url {
        author = author.icon_url(url);
    }
    author
}

pub async fn queue_commands_enabled(pool: &PgPool, server_id: i64) -> Result<bool, types::Error> {
    let record = sqlx::query!(
        "SELECT COALESCE(queue_enabled, TRUE) AS queue_enabled FROM server_bot_config WHERE server_id = $1",
        server_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(record
        .map(|row| row.queue_enabled.unwrap_or(true))
        .unwrap_or(true))
}

pub async fn pin_polls_enabled(pool: &PgPool, server_id: i64) -> Result<bool, types::Error> {
    let record = sqlx::query!(
        "SELECT COALESCE(pin_polls, TRUE) AS pin_polls FROM server_bot_config WHERE server_id = $1",
        server_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(record
        .map(|row| row.pin_polls.unwrap_or(true))
        .unwrap_or(true))
}

pub async fn auto_complete_on_deadline_enabled(
    pool: &PgPool,
    server_id: i64,
) -> Result<bool, types::Error> {
    let record = sqlx::query!(
        "SELECT COALESCE(auto_complete_on_deadline, FALSE) AS auto_complete_on_deadline FROM server_bot_config WHERE server_id = $1",
        server_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(record
        .map(|row| row.auto_complete_on_deadline.unwrap_or(false))
        .unwrap_or(false))
}

pub fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> (&str, usize) {
    if s.len() <= max_bytes {
        return (s, 0);
    }

    let mut end = max_bytes.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }

    (&s[..end], s.len() - end)
}

pub fn format_deadline(deadline: DateTime<Utc>) -> String {
    let now = Utc::now();
    let date_label = deadline.date_naive().to_string();
    let diff = deadline.signed_duration_since(now);

    if diff.num_seconds() >= 0 {
        let days = diff.num_days();
        if days > 0 {
            format!("{} (in {} days)", date_label, days)
        } else {
            let hours = diff.num_hours();
            if hours > 0 {
                format!("{} (in {} hours)", date_label, hours)
            } else {
                format!("{} (deadline is today)", date_label)
            }
        }
    } else {
        let overdue = now.signed_duration_since(deadline);
        let days_overdue = overdue.num_days();
        if days_overdue > 0 {
            format!("{} (passed {} days ago)", date_label, days_overdue)
        } else {
            let hours = overdue.num_hours();
            if hours > 0 {
                format!("{} (passed {} hours ago)", date_label, hours)
            } else {
                format!("{} (deadline has passed)", date_label)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_on_char_boundary;

    #[test]
    fn truncate_ascii_boundary() {
        let input = "abcdef";
        let (prefix, truncated) = truncate_on_char_boundary(input, 3);
        assert_eq!(prefix, "abc");
        assert_eq!(truncated, 3);
    }

    #[test]
    fn truncate_multibyte_boundary() {
        let input = "aé😊b";
        let (prefix, truncated) = truncate_on_char_boundary(input, 4);
        assert_eq!(prefix, "aé");
        assert_eq!(truncated, input.len() - prefix.len());
    }

    #[test]
    fn truncate_handles_zero_limit() {
        let input = "é";
        let (prefix, truncated) = truncate_on_char_boundary(input, 0);
        assert_eq!(prefix, "");
        assert_eq!(truncated, input.len());
    }
}
