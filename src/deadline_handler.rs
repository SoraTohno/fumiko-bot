use crate::database_helpers::finish_book_transactional;
use crate::google_books_cache::CachedGoogleBooksClient;
use crate::maturity_check::{
    channel_is_nsfw_http, create_mature_content_warning, server_maturity_enabled_by_id,
};
use crate::poll_handler;
use crate::types::Error;
use crate::util::{format_deadline, log_error, log_error_with_source, pin_polls_enabled};
use poise::serenity_prelude as serenity;
use serenity::{CreateEmbed, CreateEmbedFooter, CreateMessage, CreatePoll, CreatePollAnswer};
use sqlx::PgPool;
use sqlx::types::chrono::Utc;
use std::sync::Arc;
use tokio::time::{self, Duration};

pub fn spawn_deadline_watcher(
    http: Arc<serenity::Http>,
    pool: PgPool,
    google_books: CachedGoogleBooksClient,
) {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(600));
        loop {
            interval.tick().await;
            if let Err(err) = process_deadlines(&http, &pool, &google_books).await {
                log_error_with_source("Deadline watcher error", &err);
            }
        }
    });
}

async fn process_deadlines(
    http: &Arc<serenity::Http>,
    pool: &PgPool,
    google_books: &CachedGoogleBooksClient,
) -> Result<(), Error> {
    let rows = sqlx::query!(
        r#"
        SELECT
            scb.server_id,
            scb.volume_id,
            scb.started_at,
            scb.deadline,
            scb.announcement_channel_id,
            sbc.announcement_channel_id AS config_announcement_channel_id
        FROM server_current_book scb
        JOIN server_bot_config sbc ON sbc.server_id = scb.server_id
        WHERE scb.deadline IS NOT NULL
          AND scb.deadline <= NOW()
          AND sbc.auto_complete_on_deadline
        "#
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let server_id = row.server_id;
        let volume_id = row.volume_id.clone();

        match finish_book_transactional(pool, server_id).await {
            Ok(book_info) => {
                let completed_id = match book_info.completed_id {
                    Some(id) => id,
                    None => continue,
                };
                let started_at = book_info.started_at.unwrap_or_else(|| Utc::now());

                let volume_result = google_books.get_volume(&volume_id).await;

                let (book_title, thumbnail_url) = match &volume_result {
                    Ok(volume) => (volume.get_title(), volume.get_thumbnail_url()),
                    Err(_) => (format!("Book ({})", volume_id), None),
                };

                let duration_days = Utc::now().signed_duration_since(started_at).num_days();

                let mut embed = CreateEmbed::default()
                    .title("Deadline Reached!")
                    .field("Title", &book_title, false)
                    .field("Reading Duration", format!("{} days", duration_days.max(0)), true)
                    .description(
                        "This book reached its deadline and has been marked as finished automatically.",
                    )
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Book data from Google Books API"));

                if let Some(deadline) = row.deadline {
                    embed = embed.field("Deadline", format_deadline(deadline), true);
                }

                if let Some(url) = thumbnail_url.clone() {
                    embed = embed.image(url);
                }

                let target_channel_id = row
                    .config_announcement_channel_id
                    .or(row.announcement_channel_id)
                    .map(|id| serenity::ChannelId::new(id as u64));

                let mut poll_message_id: Option<serenity::MessageId> = None;
                let mut poll_channel_id: Option<i64> = None;

                if let Some(channel_id) = target_channel_id {
                    let can_show_volume = match &volume_result {
                        Ok(volume) => crate::maturity_check::check_volume_maturity_event(
                            http, pool, server_id, channel_id, volume,
                        )
                        .await
                        .unwrap_or(false),
                        Err(_) => true,
                    };

                    if can_show_volume {
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
                                    .expect("poll duration fits into std::time::Duration"),
                            );

                        let should_pin_poll = pin_polls_enabled(pool, server_id).await?;

                        match channel_id
                            .send_message(
                                http,
                                CreateMessage::new().embed(embed.clone()).poll(poll),
                            )
                            .await
                        {
                            Ok(message) => {
                                if let Some(poll) = message.poll.as_ref() {
                                    poll_handler::cache_rating_poll_answers(message.id, poll).await;
                                } else {
                                    log_error(
                                        "Auto-finish rating poll message missing poll payload",
                                    );
                                }

                                if should_pin_poll {
                                    if let Err(err) = message.pin(http).await {
                                        log_error_with_source(
                                            "Couldn't pin auto-finish poll message",
                                            &err,
                                        );
                                    }
                                }
                                poll_message_id = Some(message.id);
                                poll_channel_id = Some(channel_id.get() as i64);
                            }
                            Err(err) => {
                                log_error_with_source(
                                    "Failed to send auto-completion message",
                                    &err,
                                );
                            }
                        }
                    } else {
                        if let Ok(is_nsfw) = channel_is_nsfw_http(http, channel_id).await {
                            let maturity_enabled =
                                server_maturity_enabled_by_id(pool, server_id).await?;
                            let warning = create_mature_content_warning(
                                Some(&book_title),
                                is_nsfw,
                                maturity_enabled,
                            );
                            let _ = channel_id
                                .send_message(http, CreateMessage::new().embed(warning))
                                .await;

                            let notice = "A book deadline passed, but I couldn't create the rating poll because mature content isn't allowed here. Run `/finishbook` in an appropriate channel if you want to create the poll manually.";
                            let _ = channel_id.say(http, notice).await;
                        }
                    }
                }

                if let (Some(message_id), Some(channel_id_i64)) = (poll_message_id, poll_channel_id)
                {
                    let expires_at =
                        Utc::now() + chrono::Duration::days(6) + chrono::Duration::hours(23);
                    sqlx::query!(
                        "INSERT INTO rating_polls (message_id, channel_id, server_id, completed_id, expires_at)\n                        VALUES ($1, $2, $3, $4, $5)\n                        ON CONFLICT (message_id) DO NOTHING",
                        message_id.get() as i64,
                        channel_id_i64,
                        server_id,
                        completed_id,
                        expires_at
                    )
                    .execute(pool)
                    .await?;
                }
            }
            Err(err) => {
                log_error_with_source("Failed to auto-complete book", &err);
            }
        }
    }

    Ok(())
}
