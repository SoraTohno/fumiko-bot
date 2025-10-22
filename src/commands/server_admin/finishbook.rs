use crate::database_helpers::finish_book_transactional;
use crate::maturity_check::{
    check_volume_maturity, current_channel_is_nsfw, server_maturity_enabled,
};
use crate::util::{log_error, log_error_with_source, pin_polls_enabled};
use crate::{poll_handler, types::Context, types::Error};
use poise::serenity_prelude::{
    CreateEmbed, CreateEmbedFooter, CreateMessage, CreatePoll, CreatePollAnswer,
};
use sqlx::types::chrono::Utc;

#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_MESSAGES",
    description_localized(
        "en-US",
        "Mark the current book as finished and create a rating poll (requires Manage Messages)",
    ),
    user_cooldown = 10
)]
pub async fn finishbook(ctx: Context<'_>) -> Result<(), Error> {
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

    // Use the database function to finish the book
    match finish_book_transactional(pool, guild_id.get() as i64).await {
        Ok(book_info) => {
            let volume_id = book_info.volume_id.unwrap_or("Unknown".to_string());
            let started_at = book_info.started_at.unwrap();
            let completed_id = book_info.completed_id.unwrap();

            // Fetch book details from Google Books
            let volume = google_books.get_volume(&volume_id).await;

            // Check maturity if we're posting to a channel
            if let Ok(vol) = &volume {
                if !check_volume_maturity(&ctx, pool, vol).await? {
                    let is_nsfw = current_channel_is_nsfw(&ctx).await?;
                    let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;

                    if !is_nsfw || !maturity_enabled {
                        let embed = CreateEmbed::default()
                            .title("⚠️ Mature Content Warning")
                            .description(format!(
                                "The book '{}' is marked as mature content. The rating poll can only be created in an NSFW channel with mature content enabled.\n\n\
                                Consider finishing the book in an appropriate channel.",
                                vol.get_title()
                            ))
                            .color(0xB76E79)
                            .footer(CreateEmbedFooter::new("Content rating from Google Books API"));
                        ctx.send(poise::CreateReply::default().embed(embed)).await?;
                        return Ok(());
                    }
                }
            }

            let (book_title, thumbnail_url) = match volume {
                Ok(volume) => (volume.get_title(), volume.get_thumbnail_url()),
                Err(_) => (format!("Book ({})", volume_id), None),
            };

            // Calculate reading duration
            let duration = Utc::now() - started_at;
            let days = duration.num_days();

            // Create completion announcement embed
            let mut embed = CreateEmbed::default()
                .title("Book Completed!")
                .field("Title", &book_title, false)
                .field("Reading Duration", format!("{} days", days), true)
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new(
                    "Vote for your rating! • Use /select to choose the next book! • Book data from Google Books API",
                ));

            if let Some(url) = thumbnail_url {
                embed = embed.image(url);
            }

            // Create poll for rating
            let answers: Vec<CreatePollAnswer> = (1..=5)
                .map(|i| {
                    CreatePollAnswer::new()
                        .text(format!("{}/5", i))
                        .emoji("✨".to_string())
                })
                .collect();

            let poll_duration = chrono::Duration::days(6) + chrono::Duration::hours(23);

            let poll = CreatePoll::new()
                .question(format!("Rate '{}' from 1-5", book_title))
                .answers(answers)
                .duration(
                    poll_duration
                        .to_std()
                        .expect("duration fits in std::time::Duration"),
                );

            // Check if there's an announcement channel configured
            let config = sqlx::query!(
                "SELECT announcement_channel_id FROM server_bot_config WHERE server_id = $1",
                guild_id.get() as i64
            )
            .fetch_optional(pool)
            .await?;

            let channel_id = config
                .and_then(|c| c.announcement_channel_id)
                .map(|id| poise::serenity_prelude::ChannelId::new(id as u64))
                .unwrap_or_else(|| ctx.channel_id());

            let should_pin_poll = pin_polls_enabled(pool, guild_id.get() as i64).await?;

            // Send the message with embed and poll
            let message = channel_id
                .send_message(&ctx.http(), CreateMessage::new().embed(embed).poll(poll))
                .await?;

            if let Some(poll) = message.poll.as_ref() {
                poll_handler::cache_rating_poll_answers(message.id, poll).await;
            } else {
                log_error("Created rating poll message but no poll payload was returned");
            }

            if should_pin_poll {
                if let Err(err) = message.pin(&ctx.http()).await {
                    log_error_with_source("Couldn't pin poll message", &err);
                }
            }

            let expires_at = Utc::now() + poll_duration;

            // Store the poll message ID and completed_id for later processing
            sqlx::query!(
                "INSERT INTO rating_polls (message_id, channel_id, server_id, completed_id, expires_at)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (message_id) DO NOTHING",
                message.id.get() as i64,
                channel_id.get() as i64,
                guild_id.get() as i64,
                completed_id,
                expires_at
            )
            .execute(pool)
            .await?;

            let same_channel = channel_id == ctx.channel_id();

            let mut confirmation_embed =
                CreateEmbed::default().title("Poll Posted").color(0xB76E79);

            if same_channel {
                confirmation_embed = confirmation_embed.description("Poll posted in this channel.");
            } else {
                let jump_link = format!(
                    "https://discord.com/channels/{}/{}/{}",
                    guild_id.get(),
                    channel_id.get(),
                    message.id.get()
                );

                confirmation_embed = confirmation_embed.description(format!(
                    "Poll posted in <#{}>. [Jump to poll]({})",
                    channel_id.get(),
                    jump_link
                ));
            }

            ctx.send(
                poise::CreateReply::default()
                    .embed(confirmation_embed)
                    .ephemeral(true),
            )
            .await?;
        }
        Err(e) => {
            let error_msg = e.to_string();
            let embed = if error_msg.contains("No current book") {
                CreateEmbed::default()
                    .title("❌ No Current Book")
                    .description("No current book to finish. Use `/current` to check the current book. A mod/admin can use `/select` to select one.")
                    .color(0xB76E79)
            } else {
                CreateEmbed::default()
                    .title("❌ Error")
                    .description(format!("Error finishing book: {}", error_msg))
                    .color(0xB76E79)
            }.footer(CreateEmbedFooter::new("Powered by Google Books API"));

            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }
    Ok(())
}
