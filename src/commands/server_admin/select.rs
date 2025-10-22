use crate::database_helpers::select_book_transactional;
use crate::maturity_check::{
    check_volume_maturity, create_mature_content_warning, current_channel_is_nsfw,
    server_maturity_enabled,
};
use crate::types::QueryMode;
use crate::util::{
    detect_query_mode, format_deadline, get_guild_name, log_error_with_source, normalize_isbn,
    pin_polls_enabled,
};
use crate::*;
use crate::{types::Context, types::Error};
use poise::CreateReply;
use poise::futures_util::StreamExt;
use poise::serenity_prelude as serenity;
use poise::serenity_prelude::{
    CreateEmbed, CreateEmbedFooter, CreateMessage, CreatePoll, CreatePollAnswer,
};
use serenity::{
    ButtonStyle, ComponentInteractionCollector,
    builder::{
        CreateActionRow, CreateButton, CreateInteractionResponse, CreateInteractionResponseMessage,
    },
};
use sqlx::types::chrono::{DateTime, NaiveDate, TimeZone, Utc};
use std::time::Duration;

// #[derive(Clone)]
// enum PendingAction {
//     Next,
//     Random,
//     Manual { query: String, author: Option<String> },
//     Poll { size: Option<u8>, duration_hours: Option<u16> },
// }

// async fn execute_pending_action(ctx: &Context<'_>, action: PendingAction) -> Result<(), Error> {
//     match action {
//         PendingAction::Next => next(ctx.clone()).await,
//         PendingAction::Random => random(ctx.clone()).await,
//         PendingAction::Manual { query, author } => {
//             manual(ctx.clone(), query, author).await
//         }
//         PendingAction::Poll { size, duration_hours } => {
//             poll(ctx.clone(), size, duration_hours).await
//         }
//     }
// }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuardOutcome {
    NoActivePoll,     // nothing to do: proceed normally
    KeepPoll,         // user chose to keep poll (or timed out)
    CancelledProceed, // user cancelled poll; proceed with the command
}

async fn active_selection_poll_row(ctx: &Context<'_>) -> Result<Option<(i64, i64)>, Error> {
    let pool = &ctx.data().database;
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(None);
    };
    let guild_id_i64 = guild_id.get() as i64;

    // Clean up stale polls that have already expired but were never processed. These
    // stale rows violate the partial unique index on `selection_polls` and prevent
    // new polls from being created even though they are no longer active.
    sqlx::query!(
        r#"
        UPDATE selection_polls
        SET processed = TRUE
        WHERE server_id = $1
          AND NOT processed
          AND expires_at <= NOW()
        "#,
        guild_id_i64
    )
    .execute(pool)
    .await?;

    let row = sqlx::query!(
        r#"
        SELECT message_id, channel_id
        FROM selection_polls
        WHERE server_id = $1 AND NOT processed AND expires_at > NOW()
        ORDER BY expires_at DESC
        LIMIT 1
        "#,
        guild_id_i64
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| (r.message_id, r.channel_id)))
}

async fn interactive_poll_guard(ctx: &Context<'_>) -> Result<GuardOutcome, Error> {
    let Some((message_id, channel_id)) = active_selection_poll_row(ctx).await? else {
        return Ok(GuardOutcome::NoActivePoll);
    };

    let Some(guild) = ctx.guild_id() else {
        return Ok(GuardOutcome::NoActivePoll);
    };
    let guild_id = guild.get();
    let jump = format!(
        "https://discord.com/channels/{}/{}/{}",
        guild_id, channel_id as u64, message_id as u64
    );

    // Public (non-ephemeral) message so we can collect interactions reliably.
    let reply = poise::CreateReply::default()
        .content(format!(
            "⏳ A selection poll is already running.\n[Jump to poll]({})",
            jump
        ))
        .embed(
            serenity::CreateEmbed::new()
                .title("Selection poll in progress")
                .description("Do you want to cancel the current poll and proceed with your new /select action?")
                .footer(serenity::CreateEmbedFooter::new("This avoids conflicting selections")),
        )
        .components(vec![
            CreateActionRow::Buttons(vec![
                CreateButton::new("cancel_and_proceed")
                    .label("Cancel poll & proceed")
                    .style(ButtonStyle::Danger),
                CreateButton::new("keep_poll")
                    .label("Keep poll")
                    .style(ButtonStyle::Secondary),
            ])
        ]);

    let sent = ctx.send(reply).await?;
    let sent_msg = sent.message().await?;

    // Only the invoker can click; wait up to 60s.
    if let Some(mci) = ComponentInteractionCollector::new(ctx.serenity_context())
        .message_id(sent_msg.id)
        .author_id(ctx.author().id)
        .timeout(Duration::from_secs(60))
        .await
    {
        match mci.data.custom_id.as_str() {
            "cancel_and_proceed" => {
                // Mark all unprocessed selection polls for this server as processed.
                let pool = &ctx.data().database;
                sqlx::query!(
                    "UPDATE selection_polls SET processed = TRUE WHERE server_id = $1 AND NOT processed",
                    guild_id as i64
                )
                .execute(pool)
                .await?;

                // Acknowledge and remove buttons.
                let _ = mci
                    .create_response(
                        ctx.serenity_context(),
                        CreateInteractionResponse::UpdateMessage(
                            CreateInteractionResponseMessage::new()
                                .content("✅ Canceled the current selection poll. Proceeding…")
                                .components(vec![]),
                        ),
                    )
                    .await;

                return Ok(GuardOutcome::CancelledProceed);
            }
            "keep_poll" => {
                let _ = mci
                    .create_response(
                        ctx.serenity_context(),
                        CreateInteractionResponse::UpdateMessage(
                            CreateInteractionResponseMessage::new()
                                .content("🛑 Keeping the current selection poll. No changes made.")
                                .components(vec![]),
                        ),
                    )
                    .await;

                return Ok(GuardOutcome::KeepPoll);
            }
            _ => {}
        }
    } else {
        // Timeout, treat as keep
        let _ = sent
            .edit(
                *ctx,
                poise::CreateReply::default()
                    .content("⌛ Timed out; keeping the current poll.")
                    .components(vec![]),
            )
            .await;
    }

    Ok(GuardOutcome::KeepPoll)
}

fn parse_deadline_input(deadline: Option<String>) -> Result<Option<DateTime<Utc>>, String> {
    if let Some(date_str) = deadline {
        let parsed = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
            .map_err(|_| "Please use YYYY-MM-DD format (for example: 2024-05-01).".to_string())?;

        let today = Utc::now().date_naive();
        if parsed < today {
            return Err("Deadline cannot be in the past.".to_string());
        }

        let deadline_naive = parsed
            .and_hms_opt(23, 59, 59)
            .unwrap_or_else(|| parsed.and_hms_milli_opt(23, 59, 59, 999).unwrap());
        Ok(Some(Utc.from_utc_datetime(&deadline_naive)))
    } else {
        Ok(None)
    }
}

#[derive(Clone, Debug)]
struct PreloadedBookInfo {
    title: String,
    authors: String,
    thumbnail_url: Option<String>,
}

#[poise::command(
    slash_command,
    subcommands("next", "poll", "random", "manual", "remove"),
    guild_only,
    required_permissions = "MANAGE_MESSAGES",
    description_localized(
        "en-US",
        "Select a book from the queue to read (requires Manage Messages)",
    ),
    user_cooldown = 10
)]
pub async fn select(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Select the next book in the queue"),
    user_cooldown = 10
)]
async fn next(
    ctx: Context<'_>,
    #[description = "Reading deadline (YYYY-MM-DD)"] deadline: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let deadline = match parse_deadline_input(deadline) {
        Ok(value) => value,
        Err(reason) => {
            let embed = CreateEmbed::default()
                .title("❌ Invalid Deadline")
                .description(reason)
                .color(0xB76E79);
            ctx.send(CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    };

    ctx.defer().await?;

    match interactive_poll_guard(&ctx).await? {
        GuardOutcome::NoActivePoll | GuardOutcome::CancelledProceed => {
            let pool = &ctx.data().database;

            let next_book = sqlx::query!(
                r#"
                SELECT 
                    volume_id,
                    position
                FROM server_book_queue
                WHERE server_id = $1
                ORDER BY position
                LIMIT 1
                "#,
                guild_id.get() as i64
            )
            .fetch_optional(pool)
            .await?;

            match next_book {
                Some(book) => {
                    select_book(ctx, book.volume_id, deadline, None, None).await?;
                }
                None => {
                    let embed = CreateEmbed::default()
                        .title("Queue Empty")
                        .description("The queue is empty! Add books with `/queue add`.")
                        .color(0xB76E79)
                        .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                    ctx.send(CreateReply::default().embed(embed)).await?;
                }
            }
            Ok(())
        }
        GuardOutcome::KeepPoll => return Ok(()),
    }
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Create a poll to select from the queue"),
    user_cooldown = 10
)]
pub async fn poll(
    ctx: Context<'_>,
    #[description = "Number of books to include (2–10)"]
    #[min = 2]
    #[max = 10]
    size: Option<u8>,

    #[description = "How many hours the poll stays open (1–167)"]
    #[min = 1]
    #[max = 167] // < 7 days; Serenity rounds to whole hours
    duration_hours: Option<u16>,
    #[description = "Reading deadline applied to the winning book (YYYY-MM-DD)"] deadline: Option<
        String,
    >,
) -> Result<(), Error> {
    let deadline = match parse_deadline_input(deadline) {
        Ok(value) => value,
        Err(reason) => {
            let embed = CreateEmbed::default()
                .title("❌ Invalid Deadline")
                .description(reason)
                .color(0xB76E79);
            ctx.send(CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    };

    ctx.defer().await?;
    match interactive_poll_guard(&ctx).await? {
        GuardOutcome::NoActivePoll | GuardOutcome::CancelledProceed => {
            let Some(guild_id) = ctx.guild_id() else {
                let embed = CreateEmbed::default()
                    .title("❌ Error")
                    .description("This command must be used in a server.")
                    .color(0xB76E79);
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            };
            let poll_size = size.unwrap_or(5).clamp(2, 10) as i32;
            let hours = duration_hours.unwrap_or(24).min(167) as u64;
            let poll_duration = Duration::from_secs(hours * 60 * 60);

            let pool = &ctx.data().database;
            let google_books = &ctx.data().google_books;

            let current_book = sqlx::query!(
                "SELECT volume_id FROM server_current_book WHERE server_id = $1",
                guild_id.get() as i64
            )
            .fetch_optional(pool)
            .await?;

            if current_book.is_some() {
                let embed = CreateEmbed::default()
                    .title("❌ Book Already Selected")
                    .description("There's already a current book! Use `/finishbook` first to complete it, or `/select remove` to remove it.")
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            // Get books for poll
            let candidates = sqlx::query!(
                "SELECT * FROM get_queue_books_for_poll($1, $2)",
                guild_id.get() as i64,
                poll_size
            )
            .fetch_all(pool)
            .await?;

            if candidates.len() < 2 {
                let embed = CreateEmbed::default()
                    .title("❌ Insufficient Books")
                    .description("Need at least 2 books in the queue to create a poll.")
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            let book_ids: Vec<String> = candidates
                .iter()
                .map(|c| c.volume_id.clone().unwrap_or_default())
                .collect();

            // Fetch book details from Google Books
            let volumes = google_books.get_volumes_batch(&book_ids).await;

            let answer_labels: Vec<String> = candidates
                .iter()
                .enumerate()
                .map(|(i, _book)| {
                    if let Some(Ok(volume)) = volumes.get(i) {
                        format!(
                            "{}. {} — {}",
                            i + 1,
                            volume.get_title(),
                            volume.get_authors_string()
                        )
                    } else {
                        format!("{}. [Book data unavailable]", i + 1)
                    }
                })
                .collect();

            let make_poll = || {
                let answers: Vec<CreatePollAnswer> = answer_labels
                    .iter()
                    .map(|label| CreatePollAnswer::new().text(label.clone()))
                    .collect();
                CreatePoll::new()
                    .question("Pick the club's next book")
                    .answers(answers)
                    .duration(poll_duration)
            };

            let config = sqlx::query!(
                "SELECT announcement_channel_id FROM server_bot_config WHERE server_id = $1",
                guild_id.get() as i64
            )
            .fetch_optional(pool)
            .await?;

            let announcement_channel_id = config.and_then(|row| row.announcement_channel_id);
            let mut poll_channel_id = announcement_channel_id
                .map(|id| poise::serenity_prelude::ChannelId::new(id as u64))
                .unwrap_or_else(|| ctx.channel_id());

            let should_pin_poll = pin_polls_enabled(pool, guild_id.get() as i64).await?;

            let poll_content = "Cast your vote below! (Book data from Google Books API)";

            let message = match poll_channel_id
                .send_message(
                    &ctx.http(),
                    CreateMessage::new().content(poll_content).poll(make_poll()),
                )
                .await
            {
                Ok(msg) => msg,
                Err(err) => {
                    if announcement_channel_id.is_some() && poll_channel_id != ctx.channel_id() {
                        log_error_with_source("Couldn't send poll to announcement channel", &err);
                        poll_channel_id = ctx.channel_id();
                        match poll_channel_id
                            .send_message(
                                &ctx.http(),
                                CreateMessage::new().content(poll_content).poll(make_poll()),
                            )
                            .await
                        {
                            Ok(fallback_msg) => fallback_msg,
                            Err(fallback_err) => return Err(Box::new(fallback_err)),
                        }
                    } else {
                        return Err(Box::new(err));
                    }
                }
            };

            if should_pin_poll {
                if let Err(err) = message.pin(&ctx.http()).await {
                    log_error_with_source("Couldn't pin selection poll message", &err);
                }
            }

            // Store poll information
            let expires_at = Utc::now() + chrono::Duration::seconds(hours as i64 * 3600);

            sqlx::query!(
                "INSERT INTO selection_polls (message_id, channel_id, server_id, book_options, expires_at, deadline)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (message_id) DO NOTHING",
                message.id.get() as i64,
                poll_channel_id.get() as i64,
                guild_id.get() as i64,
                &book_ids,
                expires_at,
                deadline
            )
            .execute(pool)
            .await?;

            let same_channel = poll_channel_id == ctx.channel_id();

            let mut confirmation_embed =
                CreateEmbed::default().title("Poll Posted").color(0xB76E79);

            if same_channel {
                confirmation_embed = confirmation_embed.description("Poll posted in this channel.");
            } else {
                let jump_link = format!(
                    "https://discord.com/channels/{}/{}/{}",
                    guild_id.get(),
                    poll_channel_id.get(),
                    message.id.get()
                );

                confirmation_embed = confirmation_embed.description(format!(
                    "Poll posted in <#{}>. [Jump to poll]({})",
                    poll_channel_id.get(),
                    jump_link
                ));
            }

            ctx.send(
                CreateReply::default()
                    .embed(confirmation_embed)
                    .ephemeral(true),
            )
            .await?;

            Ok(())
        }
        GuardOutcome::KeepPoll => {
            return Ok(());
        }
    }
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Select a random book from the queue"),
    user_cooldown = 10
)]
async fn random(
    ctx: Context<'_>,
    #[description = "Reading deadline (YYYY-MM-DD)"] deadline: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let deadline = match parse_deadline_input(deadline) {
        Ok(value) => value,
        Err(reason) => {
            let embed = CreateEmbed::default()
                .title("❌ Invalid Deadline")
                .description(reason)
                .color(0xB76E79);
            ctx.send(CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    };

    ctx.defer().await?;
    match interactive_poll_guard(&ctx).await? {
        GuardOutcome::NoActivePoll | GuardOutcome::CancelledProceed => {
            let pool = &ctx.data().database;
            let google_books = &ctx.data().google_books;

            let random_book = sqlx::query!(
                "SELECT * FROM get_random_queue_book($1)",
                guild_id.get() as i64
            )
            .fetch_optional(pool)
            .await?;

            match random_book {
                Some(book) => {
                    let volume_id = book.volume_id.unwrap();

                    // Fetch book details
                    let volume = google_books.get_volume(&volume_id).await?;

                    if !check_volume_maturity(&ctx, pool, &volume).await? {
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

                    let pre: PreloadedBookInfo = PreloadedBookInfo {
                        title: volume.get_title(),
                        authors: volume.get_authors_string(),
                        thumbnail_url: volume.get_thumbnail_url(),
                    };
                    let book_title = volume.get_title();

                    let embed = CreateEmbed::default()
                        .title("Random Selection")
                        .description(format!(
                            "Randomly selected: **{}**\nSuggested by: {}",
                            book_title,
                            book.suggested_by_username.unwrap_or("Unknown".to_string())
                        ))
                        .color(0xB76E79)
                        .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                    ctx.send(CreateReply::default().embed(embed)).await?;

                    select_book(ctx, volume_id, deadline, None, Some(pre)).await?;
                }
                None => {
                    let embed = CreateEmbed::default()
                        .title("Queue Empty")
                        .description("The queue is empty! Add books with `/queue add`.")
                        .color(0xFFA500)
                        .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                    ctx.send(CreateReply::default().embed(embed)).await?;
                }
            }

            Ok(())
        }
        GuardOutcome::KeepPoll => return Ok(()),
    }
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Manually select a specific book"),
    user_cooldown = 10
)]
async fn manual(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13"] title_or_isbn: String,
    #[description = "Author name (optional; used when title)"] author: Option<String>,
    #[description = "User who suggested this book (defaults to queue or you)"] suggested_by: Option<
        poise::serenity_prelude::User,
    >,
    #[description = "Reading deadline (YYYY-MM-DD)"] deadline: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };
    let guild_name = get_guild_name(&ctx).await;

    let deadline = match parse_deadline_input(deadline) {
        Ok(value) => value,
        Err(reason) => {
            let embed = CreateEmbed::default()
                .title("❌ Invalid Deadline")
                .description(reason)
                .color(0xB76E79);
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    };

    ctx.defer().await?;

    match interactive_poll_guard(&ctx).await? {
        GuardOutcome::NoActivePoll | GuardOutcome::CancelledProceed => {
            let pool = &ctx.data().database;
            let google_books = &ctx.data().google_books;

            ensure_server_exists(pool, guild_id, &guild_name).await?;

            // Collect footer disclaimer notes here
            let mut footer_notes: Vec<String> = Vec::new();

            // Search for the book using the appropriate method
            let chosen = detect_query_mode(&title_or_isbn);
            let book = match chosen {
                QueryMode::Isbn => {
                    let isbn = normalize_isbn(&title_or_isbn);
                    if isbn.len() != 10 && isbn.len() != 13 {
                        let embed = CreateEmbed::default()
                            .title("❌ Invalid ISBN")
                            .description(format!(
                                "ISBN must be 10 or 13 characters long. You provided {} characters.",
                                isbn.len()
                            ))
                            .color(0xB76E79)
                            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                        ctx.send(poise::CreateReply::default().embed(embed)).await?;
                        return Ok(());
                    }
                    match google_books.search_by_isbn(&isbn).await? {
                        Some(b) => b,
                        None => {
                            let embed = CreateEmbed::default()
                                .title("❌ Book Not Found")
                                .description(format!("No book found with ISBN: {}", title_or_isbn))
                                .color(0xB76E79)
                                .footer(CreateEmbedFooter::new("Searched via Google Books API"));
                            ctx.send(poise::CreateReply::default().embed(embed)).await?;
                            return Ok(());
                        }
                    }
                }
                QueryMode::Title => {
                    let results = google_books
                        .search_books(&title_or_isbn, author.as_deref(), Some(2))
                        .await?;
                    if results.is_empty() {
                        let embed = CreateEmbed::default()
                            .title("❌ Book Not Found")
                            .description("No books found with that title.")
                            .color(0xB76E79)
                            .footer(CreateEmbedFooter::new("Searched via Google Books API"));
                        ctx.send(poise::CreateReply::default().embed(embed)).await?;
                        return Ok(());
                    }

                    let multiple = results.len() > 1;
                    let mut iter = results.into_iter(); // consume the Vec<Volume>
                    let selected = iter.next().unwrap(); // owned Volume

                    if multiple {
                        footer_notes.push("Multiple books found.".to_string());
                    }

                    selected
                }
            };

            let volume_id = &book.id;
            let pre = PreloadedBookInfo {
                title: book.get_title(),
                authors: book.get_authors_string(),
                thumbnail_url: book.get_thumbnail_url(),
            };
            let book_title = book.get_title();

            // check maturity before entering queue check
            if !check_volume_maturity(&ctx, pool, &book).await? {
                let is_nsfw = current_channel_is_nsfw(&ctx).await?;
                let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;
                let embed =
                    create_mature_content_warning(Some(&book_title), is_nsfw, maturity_enabled);
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            // Check if book is in queue and get the original suggester
            let queue_info = sqlx::query!(
                "SELECT suggested_by_user_id FROM server_book_queue WHERE server_id = $1 AND volume_id = $2",
                guild_id.get() as i64,
                volume_id
            )
            .fetch_optional(pool)
            .await?;

            let mut _fetched_user: Option<poise::serenity_prelude::User> = None;

            let suggesting_user: &poise::serenity_prelude::User = if let Some(user) =
                suggested_by.as_ref()
            {
                user
            } else if let Some(uid) = queue_info.as_ref().map(|r| r.suggested_by_user_id as u64) {
                match poise::serenity_prelude::UserId::new(uid)
                    .to_user(&ctx.http())
                    .await
                {
                    Ok(u) => {
                        _fetched_user = Some(u);
                        _fetched_user.as_ref().unwrap()
                    }
                    Err(_) => ctx.author(),
                }
            } else {
                ctx.author()
            };

            ensure_user_exists(pool, suggesting_user).await?;

            if queue_info.is_none() {
                footer_notes.push("Book was not in queue; added and selected".to_string());
                sqlx::query!(
                    "INSERT INTO server_book_queue (server_id, volume_id, suggested_by_user_id, position)
                    VALUES ($1, $2, $3, (
                        SELECT COALESCE(MAX(position), 0) + 1
                        FROM server_book_queue
                        WHERE server_id = $1
                    ))",
                    guild_id.get() as i64,
                    volume_id,
                    suggesting_user.id.get() as i64
                )
                .execute(pool)
                .await?;
            }

            let footer_disclaimer = if footer_notes.is_empty() {
                None
            } else {
                Some(footer_notes.join(" • "))
            };

            select_book(
                ctx,
                volume_id.clone(),
                deadline,
                footer_disclaimer,
                Some(pre),
            )
            .await?;

            Ok(())
        }
        GuardOutcome::KeepPoll => return Ok(()),
    }
}

// Helper function to select a book
async fn select_book(
    ctx: Context<'_>,
    volume_id: String,
    deadline: Option<DateTime<Utc>>,
    footer_disclaimer: Option<String>,
    pre: Option<PreloadedBookInfo>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };
    let guild_name = get_guild_name(&ctx).await;

    let pool = &ctx.data().database;
    let invocation_channel_id = ctx.channel_id();
    // let google_books = &ctx.data().google_books;

    ensure_server_exists(pool, guild_id, &guild_name).await?;

    // Get announcement channel if configured
    let config = sqlx::query!(
        "SELECT announcement_channel_id FROM server_bot_config WHERE server_id = $1",
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    let announcement_channel_id = config.and_then(|c| c.announcement_channel_id);

    // Use the transactional function
    let deadline_for_embed = deadline.clone();

    match select_book_transactional(
        pool,
        guild_id.get() as i64,
        &volume_id,
        announcement_channel_id,
        deadline,
    )
    .await
    {
        Ok(book_info) => {
            // Fetch book details from Google Books
            // let volume = google_books.get_volume(&volume_id).await?;
            // let book_title = volume.get_title();
            // let book_authors = volume.get_authors_string();
            let (title, authors, thumbnail_url) = if let Some(p) = pre {
                (p.title, p.authors, p.thumbnail_url)
            } else {
                let google_books = &ctx.data().google_books;
                let volume = google_books.get_volume(&volume_id).await?;

                if !check_volume_maturity(&ctx, pool, &volume).await? {
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

                (
                    volume.get_title(),
                    volume.get_authors_string(),
                    volume.get_thumbnail_url(),
                )
            };

            let suggested_by = book_info
                .suggested_by_username
                .unwrap_or("Unknown".to_string());

            // Build footer
            let mut footer_text = String::from("Book data from Google Books API");
            if let Some(extra) = footer_disclaimer.filter(|s| !s.trim().is_empty()) {
                footer_text.push_str(" • ");
                footer_text.push_str(&extra);
            }

            // Create announcement embed
            let mut embed = CreateEmbed::default()
                .title("New Book Selected!")
                .field("Title", &title, false)
                .field("Authors", &authors, false)
                .field("Suggested by", &suggested_by, false)
                .description("Happy reading! Track progress with `/progress`.")
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new(footer_text));

            if let Some(due) = deadline_for_embed {
                embed = embed.field("Deadline", format_deadline(due), true);
            }

            if let Some(thumbnail_url) = thumbnail_url {
                embed = embed.image(thumbnail_url);
            }

            let should_pin_announcements = pin_polls_enabled(pool, guild_id.get() as i64).await?;

            let mut announcement_message: Option<serenity::Message> = None;
            if let Some(channel_id) = announcement_channel_id {
                let channel = serenity::ChannelId::new(channel_id as u64);
                match channel
                    .send_message(&ctx.http(), CreateMessage::new().embed(embed.clone()))
                    .await
                {
                    Ok(message) => announcement_message = Some(message),
                    Err(err) => {
                        log_error_with_source("Couldn't send selection announcement", &err);
                    }
                }
            }

            let mut confirmation: Option<poise::ReplyHandle> = None;
            if announcement_message.is_none() {
                confirmation = Some(ctx.send(CreateReply::default().embed(embed)).await?);
            }

            let mut posted_fallback_confirmation = false;

            if should_pin_announcements {
                let mut pinned = false;

                if let Some(message) = announcement_message.as_ref() {
                    match message.pin(&ctx.http()).await {
                        Ok(_) => pinned = true,
                        Err(err) => {
                            log_error_with_source(
                                "Couldn't pin selection announcement message",
                                &err,
                            );
                        }
                    }
                }

                if !pinned {
                    if let Some(handle) = confirmation.as_ref() {
                        match handle.message().await {
                            Ok(msg) => {
                                if let Err(err) = msg.pin(&ctx.http()).await {
                                    log_error_with_source(
                                        "Couldn't pin selection confirmation message",
                                        &err,
                                    );
                                } else {
                                    pinned = true;
                                }
                            }
                            Err(err) => {
                                log_error_with_source(
                                    "Couldn't fetch selection confirmation message for pinning",
                                    &err,
                                );
                            }
                        }
                    } else if let Some(message) = announcement_message.as_ref() {
                        let fallback_jump = format!(
                            "https://discord.com/channels/{}/{}/{}",
                            guild_id.get(),
                            message.channel_id.get(),
                            message.id.get()
                        );

                        let fallback_description = if message.channel_id == invocation_channel_id {
                            format!(
                                "Announcement posted in this channel. [Jump to announcement]({})",
                                fallback_jump
                            )
                        } else {
                            format!(
                                "Announcement posted in <#{}>. [Jump to announcement]({})",
                                message.channel_id.get(),
                                fallback_jump
                            )
                        };

                        let fallback_embed = CreateEmbed::default()
                            .title("Announcement Posted")
                            .description(fallback_description)
                            .color(0xB76E79);

                        let fallback = ctx
                            .send(CreateReply::default().embed(fallback_embed))
                            .await?;

                        posted_fallback_confirmation = true;

                        match fallback.message().await {
                            Ok(msg) => {
                                if let Err(err) = msg.pin(&ctx.http()).await {
                                    log_error_with_source(
                                        "Couldn't pin fallback selection confirmation message",
                                        &err,
                                    );
                                } else {
                                    pinned = true;
                                }
                            }
                            Err(err) => {
                                log_error_with_source(
                                    "Couldn't fetch fallback selection confirmation message for pinning",
                                    &err,
                                );
                            }
                        }

                        if pinned {
                            confirmation = Some(fallback);
                        }
                    }
                }
            }

            if let Some(announcement_msg) = announcement_message {
                if !posted_fallback_confirmation {
                    let jump_link = format!(
                        "https://discord.com/channels/{}/{}/{}",
                        guild_id.get(),
                        announcement_msg.channel_id.get(),
                        announcement_msg.id.get()
                    );

                    let description = if announcement_msg.channel_id == invocation_channel_id {
                        format!(
                            "Announcement posted in this channel. [Jump to announcement]({})",
                            jump_link
                        )
                    } else {
                        format!(
                            "Announcement posted in <#{}>. [Jump to announcement]({})",
                            announcement_msg.channel_id.get(),
                            jump_link
                        )
                    };

                    let confirmation_embed = CreateEmbed::default()
                        .title("Announcement Posted")
                        .description(description)
                        .color(0xB76E79);

                    ctx.send(
                        CreateReply::default()
                            .embed(confirmation_embed)
                            .ephemeral(true),
                    )
                    .await?;
                }
            }
        }
        Err(e) => {
            let error_msg = e.to_string();
            let embed = if error_msg.contains("already has a current book") {
                CreateEmbed::default()
                    .title("❌ Book Already Selected")
                    .description("There's already a current book! Use `/finishbook` or `/select remove` first.")
                    .color(0xB76E79)
            } else if error_msg.contains("not found in queue") {
                CreateEmbed::default()
                    .title("❌ Book Not in Queue")
                    .description("This book is not in the queue.")
                    .color(0xB76E79)
            } else {
                CreateEmbed::default()
                    .title("❌ Error")
                    .description(format!("Error: {}", error_msg))
                    .color(0xB76E79)
            }.footer(CreateEmbedFooter::new("Powered by Google Books API"));

            ctx.send(CreateReply::default().embed(embed)).await?;
        }
    }

    Ok(())
}

#[poise::command(
    slash_command,
    description_localized(
        "en-US",
        "Remove the currently selected book (requires Manage Messages)",
    ),
    guild_only,
    required_permissions = "MANAGE_MESSAGES",
    user_cooldown = 10
)]
pub async fn remove(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let google_books = &ctx.data().google_books;
    let guild_id = ctx.guild_id().ok_or("No guild")?;

    let current_book = sqlx::query!(
        r#"
        SELECT scb.volume_id, du.username AS "username?"
        FROM server_current_book scb
        LEFT JOIN discord_users du ON du.user_id = scb.suggested_by_user_id
        WHERE scb.server_id = $1
        "#,
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    let Some(current_book) = current_book else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("No current book to remove.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let (book_title, book_authors) = match google_books.get_volume(&current_book.volume_id).await {
        Ok(volume) => (volume.get_title(), volume.get_authors_string()),
        Err(_) => (
            format!("Book ({})", current_book.volume_id),
            "Unknown Author".to_string(),
        ),
    };

    let mut embed = CreateEmbed::default()
        .title("🗑️ Remove current book?")
        .description(
            "This will remove the current book and clear everyone\'s reading progress for it.",
        )
        .field("Title", &book_title, false)
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Book data from Google Books API"));

    if !book_authors.is_empty() {
        embed = embed.field("Authors", &book_authors, false);
    }

    if let Some(username) = current_book.username.as_ref() {
        embed = embed.field("Suggested by", username, true);
    }

    let components = vec![CreateActionRow::Buttons(vec![
        CreateButton::new("confirm_remove")
            .label("Confirm")
            .style(ButtonStyle::Danger),
        CreateButton::new("cancel")
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ])];

    let message = ctx
        .send(CreateReply::default().embed(embed).components(components))
        .await?
        .into_message()
        .await?;

    let mut interactions = message
        .await_component_interactions(ctx)
        .timeout(Duration::from_secs(60))
        .author_id(ctx.author().id)
        .stream();

    while let Some(mci) = interactions.next().await {
        if mci.data.custom_id == "confirm_remove" {
            let row = sqlx::query!(
                r#"
                SELECT volume_id, success, error_message
                FROM remove_current_book_tx($1)
                "#,
                guild_id.get() as i64
            )
            .fetch_one(pool)
            .await?;

            if !row.success.unwrap_or(false) {
                let msg = row
                    .error_message
                    .unwrap_or_else(|| "Failed to remove current book".to_string());
                let error_embed = CreateEmbed::default()
                    .title("❌ Error")
                    .description(msg)
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Powered by Google Books API"));

                mci.create_response(
                    ctx.serenity_context(),
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .content("")
                            .components(vec![])
                            .embeds(vec![error_embed]),
                    ),
                )
                .await
                .ok();
            } else {
                let confirm_embed = CreateEmbed::default()
                    .title("✅ Current Book Removed")
                    .description(format!("Removed **{}** as the current book.", book_title))
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Book data from Google Books API"));

                mci.create_response(
                    ctx.serenity_context(),
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .content("")
                            .components(vec![])
                            .embeds(vec![confirm_embed]),
                    ),
                )
                .await
                .ok();
            }

            break;
        } else {
            mci.create_response(
                ctx.serenity_context(),
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content("❎ Cancelled.")
                        .components(vec![])
                        .embeds(vec![]),
                ),
            )
            .await
            .ok();

            break;
        }
    }

    Ok(())
}
