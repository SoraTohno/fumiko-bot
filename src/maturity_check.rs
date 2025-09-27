use crate::google_books::Volume;
use crate::types::{Context, Error};
use poise::serenity_prelude as serenity;
use poise::serenity_prelude::{ChannelId, ChannelType, CreateEmbed, CreateEmbedFooter};
use sqlx::PgPool;

// Return whether the guild has mature content enabled in settings.
pub async fn server_maturity_enabled(ctx: &Context<'_>, pool: &PgPool) -> Result<bool, Error> {
    let guild_id = match ctx.guild_id() {
        Some(id) => id,
        None => return Ok(false),
    };

    let row = sqlx::query!(
        r#"
        SELECT mature_content_enabled
        FROM server_maturity_settings
        WHERE server_id = $1
        "#,
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.mature_content_enabled).unwrap_or(false))
}

// True if the current channel is effectively NSFW, handling threads/forums.
pub async fn current_channel_is_nsfw(ctx: &Context<'_>) -> Result<bool, Error> {
    let channel_result = ctx.channel_id().to_channel(&ctx.http()).await;
    let ch = match channel_result {
        Ok(channel) => channel,
        Err(err) => {
            if let serenity::Error::Http(http_err) = &err {
                if http_err.status_code() == Some(serenity::http::StatusCode::FORBIDDEN) {
                    return Ok(false);
                }
            }
            return Err(err.into());
        }
    };

    // DMs / non-guild channels are never NSFW
    let Some(gc) = ch.guild() else {
        return Ok(false);
    };

    let is_nsfw = match gc.kind {
        // Regular text, announcement, and forum channels expose nsfw directly
        ChannelType::Text | ChannelType::News | ChannelType::Forum => gc.nsfw,

        // Threads inherit NSFW from their parent channel
        ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread => {
            if let Some(parent_id) = gc.parent_id {
                let parent_result = parent_id.to_channel(&ctx.http()).await;
                match parent_result {
                    Ok(parent) => parent.guild().map(|p| p.nsfw).unwrap_or(false),
                    Err(err) => {
                        if let serenity::Error::Http(http_err) = &err {
                            if http_err.status_code() == Some(serenity::http::StatusCode::FORBIDDEN)
                            {
                                return Ok(false);
                            }
                        }
                        return Err(err.into());
                    }
                }
            } else {
                false
            }
        }

        // Voice/stage/category/unknown: treat as not NSFW for text output
        _ => false,
    };

    Ok(is_nsfw)
}

// True if we are allowed to show mature content right now.
pub async fn can_display_mature_content(ctx: &Context<'_>, pool: &PgPool) -> Result<bool, Error> {
    // 1. Server setting must be enabled
    if !server_maturity_enabled(ctx, pool).await? {
        return Ok(false);
    }

    // 2. Channel must be 18+ (NSFW); threads inherit from their parent
    Ok(current_channel_is_nsfw(ctx).await?)
}

// Clear warning
pub fn create_mature_content_warning(
    book_title: Option<&str>,
    is_nsfw_channel: bool,
    maturity_enabled: bool,
) -> CreateEmbed {
    let title = book_title.unwrap_or("This book");

    let mut reasons: Vec<&str> = Vec::new();
    if !is_nsfw_channel {
        reasons.push(
            "• Use this command in a channel marked **NSFW (18+)** or in a thread under one.",
        );
    }
    if !maturity_enabled {
        reasons.push(
            "• An administrator must enable mature content with **`/config mature enable`**.",
        );
    }

    let mut desc = format!("**{}** is marked as containing mature content.\n\n", title);
    if !reasons.is_empty() {
        desc.push_str("**Why it was blocked / How to fix:**\n");
        desc.push_str(&reasons.join("\n"));
        desc.push_str("\n\n");
    }
    desc.push_str(
        "⚠️ **Disclaimer:** The rating comes from Google Books. The onus is on server administrators and members to ensure the maturity toggle is used responsibly and complies with Discord policies.",
    );

    CreateEmbed::default()
        .title("🔞 Mature Content Gate")
        .description(desc)
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new(
            "Content rating from Google Books API",
        ))
}

// Returns Ok(true) if the volume can be shown (either not mature, or gate passes).
pub async fn check_volume_maturity(
    ctx: &Context<'_>,
    pool: &PgPool,
    volume: &Volume,
) -> Result<bool, Error> {
    if !volume.is_mature() {
        return Ok(true);
    }
    can_display_mature_content(ctx, pool).await
}

pub async fn server_maturity_enabled_by_id(pool: &PgPool, server_id: i64) -> Result<bool, Error> {
    let row = sqlx::query!(
        r#"
        SELECT mature_content_enabled
        FROM server_maturity_settings
        WHERE server_id = $1
        "#,
        server_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.mature_content_enabled).unwrap_or(false))
}

// True if the given channel is effectively NSFW (handles threads/forums)
pub async fn channel_is_nsfw_http(
    http: &serenity::Http,
    channel_id: ChannelId,
) -> Result<bool, Error> {
    let channel_result = channel_id.to_channel(http).await;
    let ch = match channel_result {
        Ok(channel) => channel,
        Err(err) => {
            if let serenity::Error::Http(http_err) = &err {
                if http_err.status_code() == Some(serenity::http::StatusCode::FORBIDDEN) {
                    return Ok(false);
                }
            }
            return Err(err.into());
        }
    };

    // Early out for DMs / non-guild channels
    let Some(gc) = ch.guild() else {
        return Ok(false);
    };

    match gc.kind {
        // Base channel types expose nsfw directly (Option<bool>)
        ChannelType::Text | ChannelType::News | ChannelType::Forum => Ok(gc.nsfw),

        // Threads inherit NSFW from parent
        ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread => {
            if let Some(parent_id) = gc.parent_id {
                let parent_result = parent_id.to_channel(http).await;
                match parent_result {
                    Ok(parent) => Ok(parent.guild().map(|p| p.nsfw).unwrap_or(false)),
                    Err(err) => {
                        if let serenity::Error::Http(http_err) = &err {
                            if http_err.status_code() == Some(serenity::http::StatusCode::FORBIDDEN)
                            {
                                return Ok(false);
                            }
                        }
                        Err(err.into())
                    }
                }
            } else {
                Ok(false)
            }
        }

        // Other category: treat as not NSFW for posting book content
        _ => Ok(false),
    }
}

// returns Ok(true) if it's safe to show the volume
pub async fn check_volume_maturity_event(
    http: &serenity::Http,
    pool: &PgPool,
    server_id: i64,
    channel_id: ChannelId,
    volume: &Volume,
) -> Result<bool, Error> {
    if !volume.is_mature() {
        return Ok(true);
    }
    let channel_is_nsfw = channel_is_nsfw_http(http, channel_id).await?;
    let maturity_enabled = server_maturity_enabled_by_id(pool, server_id).await?;
    Ok(channel_is_nsfw && maturity_enabled)
}
