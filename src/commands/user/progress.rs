use crate::maturity_check::can_display_mature_content;
use crate::util::{embed_author_with_icon, get_guild_name};
use crate::*;
use crate::{types::Context, types::Error};
use linkify::{LinkFinder, LinkKind};
use poise::serenity_prelude::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, User,
};
use rustrict::{Censor, Type};
use sqlx::types::chrono::{DateTime, Utc};
use std::time::Duration;

use futures::StreamExt;

struct ProgressEntry {
    user_id: i64,
    username: String,
    progress_text: Option<String>,
    updated_at: Option<DateTime<Utc>>,
}

const PROGRESS_PAGE_SIZE: usize = 5;
pub(crate) const PROGRESS_HIDDEN_MESSAGE: &str = "_Progress update hidden in this channel because it contains sexual content that's not allowed here._";

fn build_progress_page_embed(
    book_title: &str,
    entries: &[ProgressEntry],
    page: usize,
    total_pages: usize,
) -> CreateEmbed {
    let mut description = String::new();

    let start = page * PROGRESS_PAGE_SIZE;
    let end = (start + PROGRESS_PAGE_SIZE).min(entries.len());

    for entry in &entries[start..end] {
        let progress_text = entry
            .progress_text
            .as_deref()
            .unwrap_or("No progress tracked yet");
        let updated_at = entry
            .updated_at
            .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        description.push_str(&format!(
            "**{}** (<@{}>)\n{}\n_Last updated: {}_\n\n",
            entry.username, entry.user_id, progress_text, updated_at
        ));
    }

    if description.is_empty() {
        description.push_str("No progress tracked yet.");
    }

    let footer_text = if total_pages > 1 {
        format!(
            "Page {}/{} • Powered by Google Books API",
            page + 1,
            total_pages
        )
    } else {
        "Powered by Google Books API".to_string()
    };

    CreateEmbed::default()
        .title(format!("📖 Reading Progress — {}", book_title))
        .description(description.trim().to_string())
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new(footer_text))
}

fn pagination_components(current_page: usize, total_pages: usize) -> Vec<CreateActionRow> {
    if total_pages <= 1 {
        return vec![];
    }

    let mut prev_button = CreateButton::new("progress_prev_page")
        .label("Previous")
        .style(ButtonStyle::Secondary);
    let mut next_button = CreateButton::new("progress_next_page")
        .label("Next")
        .style(ButtonStyle::Secondary);

    if current_page == 0 {
        prev_button = prev_button.disabled(true);
    }

    if current_page + 1 >= total_pages {
        next_button = next_button.disabled(true);
    }

    vec![CreateActionRow::Buttons(vec![prev_button, next_button])]
}

async fn is_progress_banned(
    pool: &sqlx::PgPool,
    server_id: i64,
    user_id: i64,
) -> Result<bool, sqlx::Error> {
    let banned = sqlx::query_scalar!(
        r#"
            SELECT EXISTS(
                SELECT 1
                FROM progress_command_bans
                WHERE server_id = $1 AND user_id = $2
            ) AS "exists!"
        "#,
        server_id,
        user_id
    )
    .fetch_one(pool)
    .await?;

    Ok(banned)
}

#[poise::command(
    slash_command,
    subcommands("update", "view", "clear"),
    guild_only,
    description_localized("en-US", "Track reading progress for the current book"),
    user_cooldown = 10
)]
pub async fn progress(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Validates and sanitizes progress text input
fn sexual_content_is_restricted(
    is_sexual: bool,
    is_mild: bool,
    is_moderate: bool,
    is_severe: bool,
    is_profane: bool,
) -> bool {
    if !is_sexual {
        return false;
    }

    let exception_allows_text = is_profane && is_mild && is_moderate && !is_severe;
    if exception_allows_text {
        return false;
    }

    is_severe || is_moderate
}

pub(crate) fn progress_text_is_allowed_in_channel(
    text: &str,
    allow_unrestricted_sexual: bool,
) -> bool {
    if allow_unrestricted_sexual {
        return true;
    }

    let analysis = Censor::from_str(text).analyze();
    !sexual_content_is_restricted(
        analysis.is(Type::SEXUAL),
        analysis.is(Type::MILD),
        analysis.is(Type::MODERATE),
        analysis.is(Type::SEVERE),
        analysis.is(Type::PROFANE),
    )
}

fn validate_progress_text(text: &str, allow_unrestricted_sexual: bool) -> Result<String, String> {
    // Character limit check
    if text.len() > 280 {
        return Err("Progress update must be 280 characters or less.".to_string());
    }

    // Check for inappropriate content using rustrict (allow profanity but block slurs and severe content)
    let analysis = Censor::from_str(text).analyze();
    let contains_sexual = analysis.is(Type::SEXUAL);
    let contains_severely_mean = analysis.is(Type::MEAN) && analysis.is(Type::SEVERE);
    let contains_severely_offensive = analysis.is(Type::OFFENSIVE) && analysis.is(Type::SEVERE);
    if !allow_unrestricted_sexual
        && sexual_content_is_restricted(
            contains_sexual,
            analysis.is(Type::MILD),
            analysis.is(Type::MODERATE),
            analysis.is(Type::SEVERE),
            analysis.is(Type::PROFANE),
        )
    {
        return Err(
            "Your progress update contains sexual content that can't be shared in this channel."
                .to_string(),
        );
    }
    if contains_severely_mean || contains_severely_offensive {
        return Err(
            "Your progress update contains slurs or other disallowed language.".to_string(),
        );
    }

    // Check for URLs and emails using linkify
    let finder = LinkFinder::new();
    let links: Vec<_> = finder.links(text).collect();
    if !links.is_empty() {
        // Check if any links or emails were found
        let has_url = links.iter().any(|link| link.kind() == &LinkKind::Url);
        let has_email = links.iter().any(|link| link.kind() == &LinkKind::Email);

        if has_url {
            return Err("Links and URLs are not allowed in progress updates.".to_string());
        }
        if has_email {
            return Err("Email addresses are not allowed in progress updates.".to_string());
        }
    }

    // Check for Discord pings, mentions, and special formatting
    if text.contains("<@")
        || text.contains("<#")
        || text.contains("<:")
        || text.contains("@everyone")
        || text.contains("@here")
        || text.contains("<a:")
    {
        return Err(
            "Pings, mentions, and custom emojis are not allowed in progress updates.".to_string(),
        );
    }

    // Check for file references or attachments indicators
    let file_indicators = [
        "attachment://",
        "cdn.discordapp.com",
        "media.discordapp.net",
    ];
    if file_indicators
        .iter()
        .any(|indicator| text.to_lowercase().contains(indicator))
    {
        return Err("File references are not allowed in progress updates.".to_string());
    }

    // Sanitize input - trim and clean up problematic characters
    let sanitized = text
        .trim()
        .replace('\u{200B}', "") // Remove zero-width spaces
        .replace('\u{200C}', "") // Remove zero-width non-joiner
        .replace('\u{200D}', "") // Remove zero-width joiner
        .replace('\u{FEFF}', "") // Remove byte order mark
        .replace('\r', "") // Remove carriage returns
        .replace('\n', " ") // Replace newlines with spaces
        .replace('\t', " ") // Replace tabs with spaces
        .chars()
        .filter(|c| !c.is_control() || c.is_whitespace()) // Remove control chars except whitespace
        .collect::<String>()
        .trim()
        .to_string();

    // Ensure it's not empty after sanitization
    if sanitized.is_empty() {
        return Err(
            "Progress update cannot be empty after removing invalid characters.".to_string(),
        );
    }

    // Additional length check after sanitization
    if sanitized.len() > 280 {
        return Err("Progress update is too long after processing.".to_string());
    }

    Ok(sanitized)
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Update your reading progress (280 character limit)"),
    user_cooldown = 10
)]
async fn update(
    ctx: Context<'_>,
    #[description = "Your progress update (e.g., 'Chapter 5', 'Page 123', '50% done')"]
    progress_text: String,
) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let allow_unrestricted_sexual = can_display_mature_content(&ctx, pool).await?;

    // Validate and sanitize the input
    let sanitized_progress = match validate_progress_text(&progress_text, allow_unrestricted_sexual)
    {
        Ok(text) => text,
        Err(error_msg) => {
            let embed = CreateEmbed::default()
                .title("❌ Invalid Progress Update")
                .description(error_msg)
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    };

    let google_books = &ctx.data().google_books;
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };
    if is_progress_banned(pool, guild_id.get() as i64, ctx.author().id.get() as i64).await? {
        let embed = CreateEmbed::default()
            .title("🚫 Progress Command Disabled")
            .description("You are banned from using /progress commands in this server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }
    let guild_name = get_guild_name(&ctx).await;

    ensure_user_exists(pool, ctx.author()).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    // Check if there's a current book
    let current_book = sqlx::query!(
        r#"
        SELECT 
            volume_id
        FROM server_current_book
        WHERE server_id = $1
        "#,
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    match current_book {
        Some(book) => {
            // Fetch book details from Google Books
            let book_title = match google_books.get_volume(&book.volume_id).await {
                Ok(volume) => volume.get_title(),
                Err(_) => format!("Book ({})", book.volume_id),
            };

            sqlx::query!(
                "INSERT INTO user_reading_progress (user_id, server_id, volume_id, progress_text)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (user_id, server_id) 
                 DO UPDATE SET volume_id = $3, progress_text = $4, updated_at = CURRENT_TIMESTAMP",
                ctx.author().id.get() as i64,
                guild_id.get() as i64,
                book.volume_id,
                sanitized_progress
            )
            .execute(pool)
            .await?;

            let embed = CreateEmbed::default()
                .title("✅ Progress Updated")
                .field("Book", book_title, false)
                .field("Your Progress", &sanitized_progress, false)
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));

            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
        None => {
            let embed = CreateEmbed::default()
                .title("No Current Book")
                .description("There's no current book being read in this server. Ask an admin to select one from the queue!")
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }

    Ok(())
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Clear your current reading progress"),
    user_cooldown = 10
)]
async fn clear(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let google_books = &ctx.data().google_books;
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };
    if is_progress_banned(pool, guild_id.get() as i64, ctx.author().id.get() as i64).await? {
        let embed = CreateEmbed::default()
            .title("🚫 Progress Command Disabled")
            .description("You are banned from using /progress commands in this server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let guild_name = get_guild_name(&ctx).await;

    ensure_user_exists(pool, ctx.author()).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    let current_book = sqlx::query!(
        r#"
        SELECT
            volume_id
        FROM server_current_book
        WHERE server_id = $1
        "#,
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    let Some(book) = current_book else {
        let embed = CreateEmbed::default()
            .title("No Current Book")
            .description(
                "There's no current book being read in this server, so there's no progress to clear.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let volume_id = book.volume_id;
    let book_title = match google_books.get_volume(&volume_id).await {
        Ok(volume) => volume.get_title(),
        Err(_) => format!("Book ({})", volume_id),
    };

    let result = sqlx::query!(
        "DELETE FROM user_reading_progress WHERE user_id = $1 AND server_id = $2 AND volume_id = $3",
        ctx.author().id.get() as i64,
        guild_id.get() as i64,
        volume_id
    )
    .execute(pool)
    .await?;

    if result.rows_affected() > 0 {
        let embed = CreateEmbed::default()
            .title("Progress Cleared")
            .description(format!(
                "Your reading progress for '{}' has been cleared. Use `/progress update` when you're ready to share a new update!",
                book_title
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    } else {
        let embed = CreateEmbed::default()
            .title("No Progress Found")
            .description(format!(
                "You don't have any tracked progress for '{}' right now. Use `/progress update` to share one!",
                book_title
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    }

    Ok(())
}

#[poise::command(
    slash_command,
    description_localized("en-US", "View reading progress"),
    user_cooldown = 10
)]
async fn view(
    ctx: Context<'_>,
    #[description = "User to check progress for (leave empty to view everyone)"] user: Option<User>,
) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let google_books = &ctx.data().google_books;
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };
    if is_progress_banned(pool, guild_id.get() as i64, ctx.author().id.get() as i64).await? {
        let embed = CreateEmbed::default()
            .title("🚫 Progress Command Disabled")
            .description("You are banned from using /progress commands in this server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }
    let allow_unrestricted_sexual = can_display_mature_content(&ctx, pool).await?;
    if let Some(target_user) = user.as_ref() {
        // Get current book and specific user's progress
        let result = sqlx::query!(
            r#"
            SELECT
                scb.volume_id,
                urp.progress_text,
                urp.updated_at
            FROM server_current_book scb
            LEFT JOIN user_reading_progress urp ON urp.server_id = scb.server_id
                AND urp.user_id = $1 AND urp.volume_id = scb.volume_id
            WHERE scb.server_id = $2
            "#,
            target_user.id.get() as i64,
            guild_id.get() as i64
        )
        .fetch_optional(pool)
        .await?;

        match result {
            Some(record) => {
                let book_title = match google_books.get_volume(&record.volume_id).await {
                    Ok(volume) => volume.get_title(),
                    Err(_) => format!("Book ({})", record.volume_id),
                };

                let mut embed = CreateEmbed::default()
                    .author(embed_author_with_icon(
                        format!("{}'s Reading Progress", target_user.name),
                        Some(target_user.face()),
                    ))
                    .field("Current Book", book_title, false)
                    .color(0xB76E79);

                if let Some(progress) = record.progress_text.as_ref() {
                    if progress_text_is_allowed_in_channel(progress, allow_unrestricted_sexual) {
                        embed = embed.field("Progress", progress.as_str(), false);
                    } else {
                        embed = embed.field("Progress", PROGRESS_HIDDEN_MESSAGE, false);
                    }
                    if let Some(updated) = record.updated_at {
                        embed = embed.footer(CreateEmbedFooter::new(format!(
                            "Last updated: {} • Powered by Google Books API",
                            updated.format("%Y-%m-%d %H:%M UTC")
                        )));
                    } else {
                        embed = embed.footer(CreateEmbedFooter::new("Powered by Google Books API"));
                    }
                } else {
                    embed = embed
                        .field("Progress", "No progress tracked yet", false)
                        .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                }

                ctx.send(poise::CreateReply::default().embed(embed)).await?;
            }
            None => {
                let embed = CreateEmbed::default()
                    .author(embed_author_with_icon(
                        format!("{}'s Reading Progress", target_user.name),
                        Some(target_user.face()),
                    ))
                    .title("No Current Book")
                    .description("There's no current book being read in this server.")
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
            }
        }

        return Ok(());
    }

    let current_book = sqlx::query!(
        r#"
        SELECT
            volume_id
        FROM server_current_book
        WHERE server_id = $1
        "#,
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    let Some(book) = current_book else {
        let embed = CreateEmbed::default()
            .title("No Current Book")
            .description("There's no current book being read in this server.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let book_title = match google_books.get_volume(&book.volume_id).await {
        Ok(volume) => volume.get_title(),
        Err(_) => format!("Book ({})", book.volume_id),
    };

    let progress_rows = sqlx::query!(
        r#"
        SELECT
            urp.user_id,
            du.username,
            urp.progress_text,
            urp.updated_at
        FROM user_reading_progress urp
        JOIN discord_users du ON du.user_id = urp.user_id
        WHERE urp.server_id = $1 AND urp.volume_id = $2
        ORDER BY urp.updated_at DESC NULLS LAST, urp.user_id
        "#,
        guild_id.get() as i64,
        book.volume_id
    )
    .fetch_all(pool)
    .await?;

    if progress_rows.is_empty() {
        let embed = CreateEmbed::default()
            .title(format!("Reading Progress — {}", book_title))
            .description("No one has shared their progress yet. Use `/progress update` to get things started!")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let entries: Vec<ProgressEntry> = progress_rows
        .into_iter()
        .map(|row| ProgressEntry {
            user_id: row.user_id,
            username: row.username,
            progress_text: row.progress_text.map(|text| {
                if progress_text_is_allowed_in_channel(&text, allow_unrestricted_sexual) {
                    text
                } else {
                    PROGRESS_HIDDEN_MESSAGE.to_string()
                }
            }),
            updated_at: row.updated_at,
        })
        .collect();

    let total_pages = (entries.len() + PROGRESS_PAGE_SIZE - 1) / PROGRESS_PAGE_SIZE;
    let mut current_page = 0usize;

    let mut reply = poise::CreateReply::default().embed(build_progress_page_embed(
        &book_title,
        &entries,
        current_page,
        total_pages,
    ));

    if total_pages > 1 {
        reply = reply.components(pagination_components(current_page, total_pages));
    }

    let message = ctx.send(reply).await?.into_message().await?;

    if total_pages > 1 {
        let mut interactions = message
            .await_component_interactions(ctx)
            .timeout(Duration::from_secs(120))
            .author_id(ctx.author().id)
            .stream();

        while let Some(mci) = interactions.next().await {
            let new_page = match mci.data.custom_id.as_str() {
                "progress_prev_page" if current_page > 0 => current_page - 1,
                "progress_next_page" if current_page + 1 < total_pages => current_page + 1,
                "progress_prev_page" | "progress_next_page" => current_page,
                _ => continue,
            };

            current_page = new_page;
            let embed = build_progress_page_embed(&book_title, &entries, current_page, total_pages);
            let components = pagination_components(current_page, total_pages);

            mci.create_response(
                ctx.serenity_context(),
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .embeds(vec![embed])
                        .components(components),
                ),
            )
            .await
            .ok();
        }
    }

    Ok(())
}
