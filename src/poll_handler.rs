use crate::database_helpers;
use crate::ensure_user_exists;
use crate::google_books_cache::CachedGoogleBooksClient;
use crate::maturity_check::{
    channel_is_nsfw_http, check_volume_maturity_event, create_mature_content_warning,
    server_maturity_enabled_by_id,
};
use crate::types::{Data, Error};
use crate::util::{format_deadline, pin_polls_enabled};

use poise::serenity_prelude as serenity;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::RwLock;

async fn fetch_message_with_poll_counts(
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
) -> serenity::Result<serenity::Message> {
    use serenity::http::{LightMethod, Request as HttpRequest, Route};

    let request = HttpRequest::new(
        Route::ChannelMessage {
            channel_id,
            message_id,
        },
        LightMethod::Get,
    )
    .params(Some(vec![("with_poll_counts", String::from("true"))]));

    http.fire(request).await
}

type AnswerIndexMap = HashMap<u64, i32>;
type RatingAnswerCache = HashMap<u64, AnswerIndexMap>;

static RATING_POLL_ANSWER_CACHE: OnceLock<RwLock<RatingAnswerCache>> = OnceLock::new();

fn rating_answer_cache() -> &'static RwLock<RatingAnswerCache> {
    RATING_POLL_ANSWER_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn build_rating_answer_map(poll: &serenity::Poll) -> AnswerIndexMap {
    poll.answers
        .iter()
        .enumerate()
        .map(|(idx, answer)| (answer.answer_id.get(), (idx as i32) + 1))
        .collect()
}

pub(crate) async fn cache_rating_poll_answers(
    message_id: serenity::MessageId,
    poll: &serenity::Poll,
) {
    let answer_map = build_rating_answer_map(poll);
    rating_answer_cache()
        .write()
        .await
        .insert(message_id.get(), answer_map);
}

async fn resolve_rating_choice(
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
    answer_id: serenity::AnswerId,
) -> Result<Option<i32>, Error> {
    let message_key = message_id.get();
    let answer_key = answer_id.get();

    if let Some(rating) = {
        let cache = rating_answer_cache().read().await;
        cache
            .get(&message_key)
            .and_then(|answers| answers.get(&answer_key))
            .copied()
    } {
        return Ok(Some(rating));
    }

    let message = fetch_message_with_poll_counts(http, channel_id, message_id).await?;
    let poll = match message.poll {
        Some(poll) => poll,
        None => {
            eprintln!("Poll not present on message {}", message_id);
            return Ok(None);
        }
    };

    let answer_map = build_rating_answer_map(&poll);
    let rating = answer_map.get(&answer_key).copied();

    rating_answer_cache()
        .write()
        .await
        .insert(message_key, answer_map);

    Ok(rating)
}

async fn purge_rating_poll_cache_entry(message_id: serenity::MessageId) {
    rating_answer_cache()
        .write()
        .await
        .remove(&message_id.get());
}

/// Handle non-command events via Poise's event hook.
pub async fn handle_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::GuildCreate { guild, is_new } => {
            data.guild_cache.write().await.insert(guild.id);

            if is_new.unwrap_or(false) {
                if let Err(err) = send_welcome_message(ctx, guild).await {
                    eprintln!(
                        "Failed to send welcome message to guild {}: {err}",
                        guild.id
                    );
                }
            }
        }

        serenity::FullEvent::GuildDelete { incomplete, full } => {
            let guild_id = full.as_ref().map(|guild| guild.id).unwrap_or(incomplete.id);
            data.guild_cache.write().await.remove(&guild_id);
        }

        serenity::FullEvent::MessagePollVoteAdd { event } => {
            let channel_id = event.channel_id;
            let message_id = event.message_id;
            let user_id = event.user_id;
            let answer_id = event.answer_id;

            println!(
                "Poll vote added: channel={} message={} user={} answer={}",
                channel_id, message_id, user_id, answer_id
            );

            // Check if this is a rating poll and process the vote
            process_rating_vote_add(&ctx.http, &data.database, message_id, user_id, answer_id)
                .await?;

            // Also check for completion
            check_poll_for_completion(
                &ctx.http,
                &data.database,
                &data.google_books,
                channel_id,
                message_id,
            )
            .await?;
        }

        serenity::FullEvent::MessagePollVoteRemove { event } => {
            let channel_id = event.channel_id;
            let message_id = event.message_id;
            let user_id = event.user_id;
            let answer_id = event.answer_id;

            println!(
                "Poll vote removed: channel={} message={} user={} answer={}",
                channel_id, message_id, user_id, answer_id
            );

            // Check if this is a rating poll and process the vote removal
            process_rating_vote_remove(&ctx.http, &data.database, message_id, user_id, answer_id)
                .await?;
        }

        serenity::FullEvent::MessageUpdate {
            old_if_available: _,
            new,
            event: _,
        } => {
            if let Some(new_msg) = new.as_ref() {
                let channel_id = new_msg.channel_id;
                let message_id = new_msg.id;

                // Check if poll has ended
                check_poll_for_completion(
                    &ctx.http,
                    &data.database,
                    &data.google_books,
                    channel_id,
                    message_id,
                )
                .await?;
            }
        }

        _ => {}
    }

    Ok(())
}

fn fetch_bot_member(ctx: &serenity::Context, guild: &serenity::Guild) -> Option<serenity::Member> {
    let bot_user_id = {
        let current_user = ctx.cache.current_user();
        current_user.id
    };

    guild.members.get(&bot_user_id).cloned().or_else(|| {
        ctx.cache
            .guild(guild.id)
            .and_then(|cached_guild| cached_guild.members.get(&bot_user_id).cloned())
    })
}

fn select_welcome_channel(
    ctx: &serenity::Context,
    guild: &serenity::Guild,
) -> Option<serenity::ChannelId> {
    let bot_member = fetch_bot_member(ctx, guild)?;

    let channel_has_send_permissions = |channel: &serenity::GuildChannel| {
    let perms = guild.user_permissions_in(channel, &bot_member);
    perms.contains(serenity::Permissions::VIEW_CHANNEL) 
        && perms.contains(serenity::Permissions::SEND_MESSAGES)
    };

    if let Some(channel_id) = guild.system_channel_id {
        if let Some(channel) = guild.channels.get(&channel_id) {
            if matches!(
                channel.kind,
                serenity::ChannelType::Text | serenity::ChannelType::News
            ) && channel_has_send_permissions(channel)
            {
                return Some(channel_id);
            }
        }
    }

    guild
        .channels
        .values()
        .filter(|channel| {
            matches!(
                channel.kind,
                serenity::ChannelType::Text | serenity::ChannelType::News
            ) && channel_has_send_permissions(channel)
        })
        .min_by_key(|channel| channel.position)
        .map(|channel| channel.id)
}

async fn send_welcome_message(
    ctx: &serenity::Context,
    guild: &serenity::Guild,
) -> Result<(), serenity::Error> {
    let Some(channel_id) = select_welcome_channel(ctx, guild) else {
        return Ok(());
    };

    let Some(channel) = guild.channels.get(&channel_id) else {
        return Ok(());
    };

    let Some(bot_member) = fetch_bot_member(ctx, guild) else {
        eprintln!(
            "Failed to locate bot member when sending welcome message in guild {}",
            guild.id
        );
        return Ok(());
    };

    let permissions = guild.user_permissions_in(channel, &bot_member);

    let embed = serenity::CreateEmbed::default()
        .title("Thank you for inviting Fumiko!")
        .description("It is recommended that you run the `/setup` and `/config` commands to tailor the bot to your community for things like Fumiko's announcement channel and mature content configuration.\n\n**Important:** Please make sure that Fumiko has the necessary permissions to view and send messages in any channels you intend for Fumiko to be used in!\n\nVisit [fumiko.dev/commands](https://fumiko.dev/commands) or run `/help` to view the available commands.\n\nYou can visit [fumiko.dev/guide](https://fumiko.dev/guide) for a quick explanation on how to use Fumiko bot.\n\nHappy Reading!")
        .color(0xB76E79);

    let message = serenity::CreateMessage::new().embed(embed);

    if permissions.contains(serenity::Permissions::EMBED_LINKS) {
        if let Err(err) = channel_id.send_message(&ctx.http, message).await {
            eprintln!(
                "Failed to send welcome embed to guild {} in channel {}: {err}",
                guild.id, channel_id
            );

            // Retry without the embed if embeds are disallowed or another error occurred.
            channel_id
                .send_message(
                    &ctx.http,
                    serenity::CreateMessage::new().content(
                        "Thanks for inviting Fumiko! Run `/setup` and `/config` to get started. Learn more at https://fumiko.dev.",
                    ),
                )
                .await?;
        }
    } else if permissions.contains(serenity::Permissions::SEND_MESSAGES) {
        channel_id
            .send_message(
                &ctx.http,
                serenity::CreateMessage::new().content(
                    "Thanks for inviting Fumiko! Run `/setup` and `/config` to get started. Learn more at https://fumiko.dev.",
                ),
            )
            .await?;
    } else {
        eprintln!(
            "No accessible channel found for welcome message in guild {}",
            guild.id
        );
    }

    Ok(())
}

// Process when a user adds a vote to a rating poll
async fn process_rating_vote_add(
    http: &serenity::Http,
    pool: &PgPool,
    message_id: serenity::MessageId,
    user_id: serenity::UserId,
    answer_id: serenity::AnswerId,
) -> Result<(), Error> {
    let message_id_i64 = message_id.get() as i64;
    let user_id_i64 = user_id.get() as i64;

    // Is this one of our rating polls?
    if let Some(rating_poll) = sqlx::query!(
        "SELECT completed_id, channel_id FROM rating_polls WHERE message_id = $1",
        message_id_i64
    )
    .fetch_optional(pool)
    .await?
    {
        let channel_id = serenity::ChannelId::new(rating_poll.channel_id as u64);
        let Some(rating) = resolve_rating_choice(http, channel_id, message_id, answer_id).await?
        else {
            eprintln!(
                "Could not map answer_id {} to any poll answer for message {}",
                answer_id, message_id
            );
            return Ok(());
        };

        // Ensure user exists, then upsert rating
        if let Ok(user) = user_id.to_user(http).await {
            ensure_user_exists(pool, &user).await?;
        }

        sqlx::query! {
            r#"
            INSERT INTO user_book_ratings (user_id, completed_id, rating)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id, completed_id)
            DO UPDATE SET rating = $3, rated_at = CURRENT_TIMESTAMP
            "#,
            user_id_i64,
            rating_poll.completed_id,
            rating
        }
        .execute(pool)
        .await?;

        // verify rollup updated
        if let Ok(book_info) = sqlx::query! {
            r#"
            SELECT average_rating, total_ratings
            FROM server_completed_books
            WHERE completed_id = $1
            "#,
            rating_poll.completed_id
        }
        .fetch_one(pool)
        .await
        {
            println!(
                "Book now has average rating: {:?} from {} ratings",
                book_info.average_rating,
                book_info.total_ratings.unwrap_or(0)
            );
        }
    }

    Ok(())
}

// Process when a user removes a vote from a rating poll
async fn process_rating_vote_remove(
    _http: &serenity::Http,
    pool: &PgPool,
    message_id: serenity::MessageId,
    user_id: serenity::UserId,
    _answer_id: serenity::AnswerId,
) -> Result<(), Error> {
    let message_id_i64 = message_id.get() as i64;
    let user_id_i64 = user_id.get() as i64;

    // Check if this is a rating poll
    if let Some(rating_poll) = sqlx::query!(
        "SELECT completed_id FROM rating_polls WHERE message_id = $1",
        message_id_i64
    )
    .fetch_optional(pool)
    .await?
    {
        // Remove the user's rating
        let result = sqlx::query!(
            "DELETE FROM user_book_ratings 
             WHERE user_id = $1 AND completed_id = $2",
            user_id_i64,
            rating_poll.completed_id
        )
        .execute(pool)
        .await;

        match result {
            Ok(rows) => {
                if rows.rows_affected() > 0 {
                    println!(
                        "Removed rating for user {} for completed_id {}",
                        user_id, rating_poll.completed_id
                    );

                    // The trigger should automatically update the average rating
                    if let Ok(book_info) = sqlx::query!(
                        "SELECT average_rating, total_ratings FROM server_completed_books 
                         WHERE completed_id = $1",
                        rating_poll.completed_id
                    )
                    .fetch_one(pool)
                    .await
                    {
                        println!(
                            "Book now has average rating: {:?} from {} ratings",
                            book_info.average_rating,
                            book_info.total_ratings.unwrap_or(0)
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("Error removing rating: {}", e);
            }
        }
    }

    Ok(())
}

async fn handle_missing_poll_message(
    pool: &PgPool,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
    error_message: &str,
) -> Result<(), Error> {
    let message_id_i64 = message_id.get() as i64;

    if let Some(rating_poll) = sqlx::query!(
        "SELECT server_id, processed FROM rating_polls WHERE message_id = $1",
        message_id_i64
    )
    .fetch_optional(pool)
    .await?
    {
        let already_processed = rating_poll.processed.unwrap_or(false);

        if already_processed {
            println!(
                "Rating poll message {} in channel {} already processed when fetch failed (server {}): {}",
                message_id, channel_id, rating_poll.server_id, error_message
            );
        } else {
            sqlx::query!(
                "UPDATE rating_polls SET processed = TRUE WHERE message_id = $1",
                message_id_i64
            )
            .execute(pool)
            .await?;

            println!(
                "Rating poll message {} in channel {} closed after fetch failure (server {}): {}",
                message_id, channel_id, rating_poll.server_id, error_message
            );
        }

        purge_rating_poll_cache_entry(message_id).await;
        return Ok(());
    }

    if let Some(selection_poll) = sqlx::query!(
        "SELECT server_id, processed, selected_volume_id FROM selection_polls WHERE message_id = $1",
        message_id_i64
    )
    .fetch_optional(pool)
    .await?
    {
        let already_processed = selection_poll.processed.unwrap_or(false);

        if !already_processed {
            sqlx::query!(
                "UPDATE selection_polls SET processed = TRUE, selected_volume_id = NULL WHERE message_id = $1",
                message_id_i64
            )
            .execute(pool)
            .await?;
        }

        if already_processed {
            println!(
                "Selection poll message {} in channel {} already processed when fetch failed (server {}): {}",
                message_id,
                channel_id,
                selection_poll.server_id,
                error_message
            );
        } else if selection_poll.selected_volume_id.is_some() {
            println!(
                "Selection poll message {} in channel {} closed and cleared stale winner after fetch failure (server {}): {}",
                message_id,
                channel_id,
                selection_poll.server_id,
                error_message
            );
        } else {
            println!(
                "Selection poll message {} in channel {} closed after fetch failure (server {}): {}",
                message_id,
                channel_id,
                selection_poll.server_id,
                error_message
            );
        }

        return Ok(());
    }

    println!(
        "Poll message {} in channel {} could not be fetched and no database row was found: {}",
        message_id, channel_id, error_message
    );

    Ok(())
}

/// Helper to check if a poll has completed
async fn check_poll_for_completion(
    http: &serenity::Http,
    pool: &PgPool,
    google_books: &CachedGoogleBooksClient,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
) -> Result<(), Error> {
    match fetch_message_with_poll_counts(http, channel_id, message_id).await {
        Ok(message) => {
            if let Some(poll) = &message.poll {
                if let Some(results) = &poll.results {
                    // Check if poll has ended
                    if results.is_finalized {
                        process_poll_completion(
                            http,
                            pool,
                            google_books,
                            channel_id,
                            message_id,
                            poll,
                        )
                        .await?;
                    }
                }
            }
        }
        Err(err) => {
            let err_message = err.to_string();
            handle_missing_poll_message(pool, channel_id, message_id, &err_message).await?;
            return Ok(());
        }
    }
    Ok(())
}

/// Public wrapper that callers can use without a `Context`
pub async fn check_poll_for_completion_public(
    http: &serenity::Http,
    pool: &PgPool,
    google_books: &CachedGoogleBooksClient,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
) -> Result<(), Error> {
    check_poll_for_completion(http, pool, google_books, channel_id, message_id).await
}

// Process poll completion
async fn process_poll_completion(
    http: &serenity::Http,
    pool: &PgPool,
    google_books: &CachedGoogleBooksClient,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
    poll: &serenity::Poll,
) -> Result<(), Error> {
    let message_id_i64 = message_id.get() as i64;

    // Check if it's a rating poll
    if let Some(rating_poll) = sqlx::query!(
        "SELECT completed_id, channel_id, server_id FROM rating_polls 
         WHERE message_id = $1 AND NOT processed",
        message_id_i64
    )
    .fetch_optional(pool)
    .await?
    {
        // Mark as processed
        sqlx::query!(
            "UPDATE rating_polls SET processed = TRUE WHERE message_id = $1",
            message_id_i64
        )
        .execute(pool)
        .await?;

        // Get final rating stats
        if let Ok(book_stats) = sqlx::query!(
            r#"
            SELECT 
                scb.volume_id,
                scb.average_rating,
                scb.total_ratings
            FROM server_completed_books scb
            WHERE scb.completed_id = $1
            "#,
            rating_poll.completed_id
        )
        .fetch_one(pool)
        .await
        {
            // Fetch book title from Google Books
            let book_title = match google_books.get_volume(&book_stats.volume_id).await {
                Ok(volume) => volume.get_title(),
                Err(_) => format!("Book ({})", book_stats.volume_id),
            };

            // Send a summary message
            let channel = serenity::ChannelId::new(rating_poll.channel_id as u64);
            let mut embed = serenity::CreateEmbed::default()
                .title("Rating Poll Complete")
                .field("Book", &book_title, false)
                .color(0xB76E79)
                .footer(serenity::CreateEmbedFooter::new(
                    "Book data from Google Books API",
                ));

            if let Some(avg) = book_stats.average_rating {
                embed = embed
                    .field("Average Rating", format!("{:.1}/5 ✨", avg), true)
                    .field(
                        "Total Votes",
                        book_stats.total_ratings.unwrap_or(0).to_string(),
                        true,
                    );
            } else {
                embed = embed.description("No ratings were submitted.");
            }

            let _ = channel
                .send_message(http, serenity::CreateMessage::new().embed(embed))
                .await;
        }

        println!(
            "Rating poll {} has been processed and finalized.",
            message_id
        );
        purge_rating_poll_cache_entry(message_id).await;
        return Ok(());
    }

    // Check if it's a selection poll
    if let Some(selection_poll) = sqlx::query!(
        r#"
        SELECT server_id, book_options, deadline
        FROM selection_polls
        WHERE message_id = $1 AND NOT processed
        "#,
        message_id_i64
    )
    .fetch_optional(pool)
    .await?
    {
        if let Some(results) = &poll.results {
            // Find all answers with max votes (handles ties)
            let mut max_votes: u64 = 0;
            let mut winning_indices: Vec<usize> = Vec::new();

            for ac in &results.answer_counts {
                // Map answer_id to index immediately
                if let Some(idx) = poll.answers.iter().position(|a| a.answer_id == ac.id) {
                    if ac.count > max_votes {
                        max_votes = ac.count;
                        winning_indices = vec![idx];
                    } else if ac.count == max_votes && ac.count > 0 {
                        winning_indices.push(idx);
                    }
                }
            }

            // Handle no votes
            if winning_indices.is_empty() || max_votes == 0 {
                let embed = serenity::CreateEmbed::default()
                    .title("Poll Complete")
                    .description("Poll ended with no votes, so no book was selected.")
                    .color(0xB76E79);

                let _ = channel_id
                    .send_message(http, serenity::CreateMessage::new().embed(embed))
                    .await;

                sqlx::query!(
                    "UPDATE selection_polls SET processed = TRUE WHERE message_id = $1",
                    message_id_i64
                )
                .execute(pool)
                .await?;

                return Ok(());
            }

            // Handle tie by picking the earliest in queue
            let winning_index = if winning_indices.len() > 1 {
                let embed = serenity::CreateEmbed::default()
                    .title("Poll Tie")
                    .description(format!(
                        "Poll ended in a tie with {} votes each. Selecting the book earliest in the queue.",
                        max_votes
                    ))
                    .color(0xB76E79);

                let _ = channel_id
                    .send_message(http, serenity::CreateMessage::new().embed(embed))
                    .await;
                *winning_indices.iter().min().unwrap()
            } else {
                winning_indices[0]
            };

            // Validate index bounds
            if winning_index >= selection_poll.book_options.len() {
                eprintln!("Warning: Winning index {} out of bounds", winning_index);
                sqlx::query!(
                    "UPDATE selection_polls SET processed = TRUE WHERE message_id = $1",
                    message_id_i64
                )
                .execute(pool)
                .await?;
                return Ok(());
            }

            let winning_volume_id = &selection_poll.book_options[winning_index];

            // Fetch book for messaging + maturity check. If this fails we continue with
            // fallback metadata so the selection still succeeds.
            let volume = match google_books.get_volume(winning_volume_id).await {
                Ok(v) => Some(v),
                Err(e) => {
                    eprintln!(
                        "Failed to fetch volume {} for maturity check: {}",
                        winning_volume_id, e
                    );
                    None
                }
            };

            let (book_title, book_authors) = if let Some(volume) = volume.as_ref() {
                let authors = volume.get_authors_string();
                let authors = if authors.trim().is_empty() {
                    "Unknown author".to_string()
                } else {
                    authors
                };
                (volume.get_title(), authors)
            } else {
                (
                    format!("Book ({})", winning_volume_id),
                    "Unknown author".to_string(),
                )
            };

            // Maturity gate (post warning and close poll if not allowed)
            if let Some(volume) = volume.as_ref() {
                let can_show = check_volume_maturity_event(
                    http,
                    pool,
                    selection_poll.server_id,
                    channel_id,
                    volume,
                )
                .await?;

                if !can_show {
                    let is_nsfw = channel_is_nsfw_http(http, channel_id).await?;
                    let maturity_enabled =
                        server_maturity_enabled_by_id(pool, selection_poll.server_id).await?;
                    let embed =
                        create_mature_content_warning(Some(&book_title), is_nsfw, maturity_enabled);
                    let _ = channel_id
                        .send_message(http, serenity::CreateMessage::new().embed(embed))
                        .await;

                    // Important: mark processed so we don't block manual selection
                    sqlx::query!(
                        "UPDATE selection_polls SET processed = TRUE WHERE message_id = $1",
                        message_id_i64
                    )
                    .execute(pool)
                    .await?;

                    return Ok(());
                }
            }

            // Try to apply the winner
            //    We update the poll to processed only after a terminal outcome.
            let config = sqlx::query!(
                "SELECT announcement_channel_id FROM server_bot_config WHERE server_id = $1",
                selection_poll.server_id
            )
            .fetch_optional(pool)
            .await?;

            let announcement_channel_id = config.and_then(|c| c.announcement_channel_id);
            let poll_deadline = selection_poll.deadline.clone();

            match database_helpers::select_book_transactional(
                pool,
                selection_poll.server_id,
                winning_volume_id,
                announcement_channel_id,
                poll_deadline.clone(),
            )
            .await
            {
                Ok(book_info) => {
                    // Success: announce and close the poll
                    let should_pin_announcements =
                        pin_polls_enabled(pool, selection_poll.server_id).await?;

                    let target_channel = announcement_channel_id
                        .map(|chan| serenity::ChannelId::new(chan as u64))
                        .unwrap_or(channel_id);
                    let suggested_by = book_info
                        .suggested_by_username
                        .unwrap_or_else(|| "Unknown".to_string());

                    let mut footer_text = String::from("Book data from Google Books API");
                    if volume.is_none() {
                        footer_text.push_str(
                            " • Google Books data couldn't be loaded; information may be incomplete.",
                        );
                    }

                    let mut embed = serenity::CreateEmbed::default()
                        .title("New Book Selected!")
                        .field("Title", &book_title, false)
                        .field("Authors", &book_authors, false)
                        .field("Suggested by", &suggested_by, false)
                        .description("Happy reading! Track progress with `/progress`.")
                        .color(0xB76E79)
                        .footer(serenity::CreateEmbedFooter::new(footer_text));

                    if let Some(deadline) = poll_deadline {
                        embed = embed.field("Deadline", format_deadline(deadline), true);
                    }

                    if let Some(volume) = volume.as_ref() {
                        if let Some(thumbnail_url) = volume.get_thumbnail_url() {
                            embed = embed.image(thumbnail_url);
                        }
                    }

                    let mut announcement_message: Option<serenity::Message> = None;

                    match target_channel
                        .send_message(http, serenity::CreateMessage::new().embed(embed.clone()))
                        .await
                    {
                        Ok(msg) => announcement_message = Some(msg),
                        Err(err) => {
                            eprintln!(
                                "Couldn't send selection announcement to channel {}: {}",
                                target_channel.get(),
                                err
                            );

                            if target_channel != channel_id {
                                match channel_id
                                    .send_message(
                                        http,
                                        serenity::CreateMessage::new().embed(embed.clone()),
                                    )
                                    .await
                                {
                                    Ok(msg) => announcement_message = Some(msg),
                                    Err(fallback_err) => {
                                        eprintln!(
                                            "Couldn't send selection fallback announcement to channel {}: {}",
                                            channel_id, fallback_err
                                        );
                                    }
                                }
                            }
                        }
                    }

                    if should_pin_announcements {
                        if let Some(message) = announcement_message.as_ref() {
                            if let Err(err) = message.pin(http).await {
                                eprintln!("Couldn't pin selection announcement message: {err}");
                            }
                        }
                    }

                    sqlx::query!(
                        "UPDATE selection_polls
                         SET processed = TRUE, selected_volume_id = $1
                         WHERE message_id = $2",
                        winning_volume_id,
                        message_id_i64
                    )
                    .execute(pool)
                    .await?;
                }
                Err(e) => {
                    let msg = e.to_string();
                    let (title, description) = if msg.contains("already has a current book") {
                        (
                            "Poll Ended",
                            format!(
                                "**{}** won with {} votes, but another book was already selected. Use `/finishbook` first if you want to change books.",
                                book_title, max_votes
                            ),
                        )
                    } else if msg.contains("not found in queue") {
                        (
                            "Poll Ended",
                            format!(
                                "**{}** won with {} votes, but was removed from the queue. Please run a new poll.",
                                book_title, max_votes
                            ),
                        )
                    } else {
                        (
                            "❌ Selection Failed",
                            format!(
                                "Couldn't select **{}** (winner with {} votes): {}",
                                book_title, max_votes, msg
                            ),
                        )
                    };

                    let embed = serenity::CreateEmbed::default()
                        .title(title)
                        .description(description)
                        .color(0xB76E79);

                    let _ = channel_id
                        .send_message(http, serenity::CreateMessage::new().embed(embed))
                        .await;

                    // Still mark as processed
                    sqlx::query!(
                        "UPDATE selection_polls SET processed = TRUE WHERE message_id = $1",
                        message_id_i64
                    )
                    .execute(pool)
                    .await?;
                }
            }
        }
    }
    Ok(())
}
