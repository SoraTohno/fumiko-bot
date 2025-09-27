use crate::commands::user::progress::{
    PROGRESS_HIDDEN_MESSAGE, progress_text_is_allowed_in_channel,
};
use crate::maturity_check::{
    can_display_mature_content, check_volume_maturity, create_mature_content_warning,
    current_channel_is_nsfw, server_maturity_enabled,
};
use crate::util::{embed_author_with_icon, format_deadline, get_guild_icon_url, get_guild_name};
use crate::{types::Context, types::Error};
use chrono::Utc;
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter};

#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "Show the book currently being read by the club"),
    user_cooldown = 10
)]
pub async fn current(ctx: Context<'_>) -> Result<(), Error> {
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
    let guild_name = get_guild_name(&ctx).await;
    let guild_icon = get_guild_icon_url(&ctx).await;

    // Get current book and member progress (UPDATED to include suggested_by)
    let current_book = sqlx::query!(
        r#"
        SELECT
            scb.volume_id,
            scb.started_at,
            scb.deadline,
            du.username as "suggested_by?",
            COUNT(DISTINCT urp.user_id) as members_tracking,
            MAX(urp.updated_at) as last_progress_update
        FROM server_current_book scb
        LEFT JOIN discord_users du ON du.user_id = scb.suggested_by_user_id
        LEFT JOIN user_reading_progress urp ON urp.server_id = scb.server_id AND urp.volume_id = scb.volume_id
        WHERE scb.server_id = $1
        GROUP BY scb.volume_id, scb.started_at, scb.deadline, du.username
        "#,
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    match current_book {
        Some(book) => {
            let started_at = book.started_at.unwrap();
            let members_tracking = book.members_tracking.unwrap_or(0);

            // Fetch book details from Google Books
            let volume_result = google_books.get_volume(&book.volume_id).await;

            // Check maturity if we successfully fetched the volume
            if let Ok(volume) = &volume_result {
                if !check_volume_maturity(&ctx, pool, volume).await? {
                    let is_nsfw = current_channel_is_nsfw(&ctx).await?;
                    let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;
                    let embed = create_mature_content_warning(
                        Some(&volume.get_title()),
                        is_nsfw,
                        maturity_enabled,
                    );
                    ctx.send(poise::CreateReply::default().embed(embed)).await?;
                    return Ok(());
                }
            }

            let (title, authors, thumbnail_url) = match volume_result {
                Ok(volume) => (
                    volume.get_title(),
                    volume.get_authors_string(),
                    volume.get_thumbnail_url(),
                ),
                Err(_) => (
                    format!("Book ({})", book.volume_id),
                    "Unknown Author".to_string(),
                    None,
                ),
            };

            // Calculate days since started
            let days = Utc::now().signed_duration_since(started_at).num_days();

            let mut embed = CreateEmbed::default()
                .author(embed_author_with_icon(
                    format!("{} Current Book", guild_name),
                    guild_icon.clone(),
                ))
                .field("Title", &title, false)
                .field("Author(s)", &authors, false)
                .field(
                    "Suggested by",
                    book.suggested_by.as_deref().unwrap_or("Unknown"),
                    true,
                )
                .field("Started", format!("{} days ago", days), true)
                .field(
                    "Members Tracking Progress",
                    members_tracking.to_string(),
                    true,
                )
                .color(0xB76E79);

            if let Some(deadline) = book.deadline {
                embed = embed.field("Deadline", format_deadline(deadline), true);
            }

            if let Some(url) = thumbnail_url {
                embed = embed.image(url);
            }

            // Determine whether mature content can be displayed in this channel
            let allow_unrestricted_sexual = can_display_mature_content(&ctx, pool).await?;

            // Get recent progress updates
            let recent_progress = sqlx::query!(
                r#"
                SELECT
                    du.username,
                    urp.progress_text,
                    urp.updated_at
                FROM user_reading_progress urp
                JOIN discord_users du ON du.user_id = urp.user_id
                WHERE urp.server_id = $1 AND urp.volume_id = $2
                ORDER BY urp.updated_at DESC
                LIMIT 3
                "#,
                guild_id.get() as i64,
                book.volume_id
            )
            .fetch_all(pool)
            .await?;

            if !recent_progress.is_empty() {
                let mut progress_text = String::new();
                for progress in recent_progress {
                    let display_text = match progress.progress_text {
                        Some(text) => {
                            if progress_text_is_allowed_in_channel(&text, allow_unrestricted_sexual)
                            {
                                text
                            } else {
                                PROGRESS_HIDDEN_MESSAGE.to_string()
                            }
                        }
                        None => "No progress".to_string(),
                    };
                    progress_text
                        .push_str(&format!("**{}**: {}\n", progress.username, display_text));
                }
                embed = embed.field("Recent Progress", progress_text, false);
            }

            // Add footer with last update time and Google Books attribution
            if let Some(last_update) = book.last_progress_update {
                let update_duration = Utc::now() - last_update;
                let hours = update_duration.num_hours();
                embed = embed.footer(CreateEmbedFooter::new(format!(
                    "Last progress update: {} hours ago • Powered by Google Books API",
                    hours
                )));
            } else {
                embed = embed.footer(CreateEmbedFooter::new("Powered by Google Books API"));
            }

            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
        None => {
            let embed = CreateEmbed::default()
                .author(embed_author_with_icon(
                    format!("{} Current Book", guild_name),
                    guild_icon,
                ))
                .title("No Current Book")
                .description("There's no book currently being read. Ask an admin to select one with `/select`!")
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));

            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }

    Ok(())
}
