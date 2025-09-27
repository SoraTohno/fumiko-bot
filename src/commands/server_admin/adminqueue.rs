use crate::maturity_check::{
    check_volume_maturity, create_mature_content_warning, current_channel_is_nsfw,
    server_maturity_enabled,
};
use crate::types::QueryMode;
use crate::util::{detect_query_mode, get_guild_name, normalize_isbn};
use crate::*;
use crate::{types::Context, types::Error};
use poise::futures_util::StreamExt;
use poise::serenity_prelude::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter, User,
};
use sqlx::Row;
use std::time::Duration;

#[derive(poise::ChoiceParameter, Clone, Copy, Debug, Eq, PartialEq)]
enum QueueInsertion {
    #[name = "front"]
    Front,
    #[name = "back"]
    Back,
}

#[poise::command(
    slash_command,
    subcommands("add", "remove"),
    guild_only,
    required_permissions = "MANAGE_MESSAGES",
    description_localized("en-US", "Admin queue management (requires Manage Messages)"),
    user_cooldown = 10
)]
pub async fn adminqueue(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Add a book to the queue (admin - no limits)"),
    user_cooldown = 10,
    guild_only
)]
async fn add(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13"] title_or_isbn: String,
    #[description = "User who suggested this book (defaults to you)"] suggested_by: Option<User>,
    #[description = "Author name (optional; used when searching by title)"] author: Option<String>,
    #[description = "Place the book at the front or back of the queue"] placement: Option<
        QueueInsertion,
    >,
) -> Result<(), Error> {
    ctx.defer().await?;

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
    let google_books = &ctx.data().google_books;

    // Use the specified user or default to the command invoker
    let suggesting_user = suggested_by.as_ref().unwrap_or_else(|| ctx.author());

    ensure_user_exists(pool, suggesting_user).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    let placement = placement.unwrap_or(QueueInsertion::Back);

    // Collect footer disclaimer notes here
    let mut footer_notes: Vec<String> = Vec::new();

    // Search for the book
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
                .search_books(&title_or_isbn, author.as_deref(), Some(5))
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

            let selected = results.clone().into_iter().next().unwrap();

            if results.len() > 1 {
                footer_notes.push("Multiple books found.".to_string());
            }

            selected
        }
    };

    // Check maturity
    if !check_volume_maturity(&ctx, pool, &book).await? {
        let is_nsfw = current_channel_is_nsfw(&ctx).await?;
        let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;
        let embed =
            create_mature_content_warning(Some(&book.get_title()), is_nsfw, maturity_enabled);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let volume_id = &book.id;
    let book_title = book.get_title();
    let book_authors = book.get_authors_string();

    // Check if THIS EXACT book is already in queue
    let already_queued = sqlx::query!(
        "SELECT du.username FROM server_book_queue sbq
         JOIN discord_users du ON du.user_id = sbq.suggested_by_user_id
         WHERE sbq.server_id = $1 AND sbq.volume_id = $2",
        guild_id.get() as i64,
        volume_id
    )
    .fetch_optional(pool)
    .await?;

    if let Some(existing) = already_queued {
        let embed = CreateEmbed::default()
            .title("Already in Queue")
            .description(format!(
                "This book is already in the queue (suggested by {})!",
                existing.username
            ))
            .field("Title", &book_title, false)
            .field("Authors", &book_authors, false)
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Add to queue WITHOUT checking for user's existing books (admin can add unlimited)
    let new_position = match placement {
        QueueInsertion::Back => {
            let record = sqlx::query!(
                "INSERT INTO server_book_queue (server_id, volume_id, suggested_by_user_id, position)
                VALUES ($1, $2, $3, (
                    SELECT COALESCE(MAX(position), 0) + 1
                    FROM server_book_queue
                    WHERE server_id = $1
                ))
                RETURNING position",
                guild_id.get() as i64,
                volume_id,
                suggesting_user.id.get() as i64
            )
            .fetch_one(pool)
            .await?;

            record.position
        }
        QueueInsertion::Front => {
            let mut tx = pool.begin().await?;

            sqlx::query!(
                "UPDATE server_book_queue SET position = position + 1 WHERE server_id = $1",
                guild_id.get() as i64
            )
            .execute(&mut *tx)
            .await?;

            let inserted = sqlx::query!(
                "INSERT INTO server_book_queue (server_id, volume_id, suggested_by_user_id, position)
                VALUES ($1, $2, $3, 1)
                RETURNING position",
                guild_id.get() as i64,
                volume_id,
                suggesting_user.id.get() as i64
            )
            .fetch_one(&mut *tx)
            .await?;

            let position = inserted.position;
            tx.commit().await?;
            position
        }
    };

    let mut embed = CreateEmbed::default()
        .title("✅ Book Added to Queue (Admin)")
        .field("Title", &book_title, false)
        .field("Authors", &book_authors, false)
        .field("Suggested by", &suggesting_user.name, true)
        .field("Position", format!("#{}", new_position), true)
        .color(0xB76E79);

    if let Some(thumbnail_url) = book.get_thumbnail_url() {
        embed = embed.thumbnail(thumbnail_url);
    }

    // Build footer with optional disclaimers
    let mut footer_text = String::from("Book data from Google Books API");
    if !footer_notes.is_empty() {
        footer_text.push_str(" • ");
        footer_text.push_str(&footer_notes.join(" • "));
    }
    embed = embed.footer(CreateEmbedFooter::new(footer_text));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    subcommands("book", "user"),
    description_localized("en-US", "Remove a book from the queue"),
    user_cooldown = 10,
    guild_only
)]
async fn remove(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "book",
    description_localized("en-US", "Remove a book by title or ISBN"),
    user_cooldown = 10
)]
async fn book(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13"] title_or_isbn: String,
    #[description = "Author name (optional; used when searching by title)"] author: Option<String>,
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

    // Search for the book
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
                .search_books(&title_or_isbn, author.as_deref(), Some(10))
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

            // Check which results are actually in the queue
            let volume_ids: Vec<String> = results.iter().map(|v| v.id.clone()).collect();
            let placeholders: Vec<String> = (2..volume_ids.len() + 2)
                .map(|i| format!("${}", i))
                .collect();

            let query_str = format!(
                "SELECT volume_id FROM server_book_queue 
                 WHERE server_id = $1 AND volume_id IN ($2{})",
                if !placeholders.is_empty() {
                    format!(", {}", placeholders.join(", "))
                } else {
                    String::new()
                }
            );

            let mut query = sqlx::query(&query_str).bind(guild_id.get() as i64);
            for vid in &volume_ids {
                query = query.bind(vid);
            }

            let queued_ids: Vec<String> = query
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(|row| row.get("volume_id"))
                .collect();

            if queued_ids.is_empty() {
                let embed = CreateEmbed::default()
                    .title("❌ Not in Queue")
                    .description("None of the matching books are in the queue.")
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            // Find the first result that's actually in the queue
            results
                .into_iter()
                .find(|v| queued_ids.contains(&v.id))
                .unwrap()
        }
    };

    let volume_id = &book.id;
    let book_title = book.get_title();
    let book_authors = book.get_authors_string();

    // Get book details from queue
    let queue_book = sqlx::query!(
        r#"
        SELECT 
            du.username as suggested_by
        FROM server_book_queue sbq
        JOIN discord_users du ON du.user_id = sbq.suggested_by_user_id
        WHERE sbq.server_id = $1 AND sbq.volume_id = $2
        "#,
        guild_id.get() as i64,
        volume_id
    )
    .fetch_optional(pool)
    .await?;

    match queue_book {
        Some(book_info) => {
            // Create confirmation embed
            let embed = CreateEmbed::default()
                .title("🗑️ Remove book from queue?")
                .field("Title", &book_title, false)
                .field("Authors", &book_authors, false)
                .field("Suggested by", &book_info.suggested_by, false)
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));

            let components = vec![CreateActionRow::Buttons(vec![
                CreateButton::new("confirm_remove")
                    .label("Confirm")
                    .style(ButtonStyle::Danger),
                CreateButton::new("cancel")
                    .label("Cancel")
                    .style(ButtonStyle::Secondary),
            ])];

            let mut msg = ctx
                .send(
                    poise::CreateReply::default()
                        .embed(embed)
                        .components(components),
                )
                .await?
                .into_message()
                .await?;

            // Wait for button interaction
            let collector = msg
                .await_component_interactions(ctx.serenity_context())
                .author_id(ctx.author().id)
                .timeout(Duration::from_secs(30));

            let mut stream = collector.stream();
            if let Some(mci) = stream.next().await {
                if mci.data.custom_id == "confirm_remove" {
                    // Remove from queue
                    sqlx::query!(
                        "DELETE FROM server_book_queue 
                         WHERE server_id = $1 AND volume_id = $2",
                        guild_id.get() as i64,
                        volume_id
                    )
                    .execute(pool)
                    .await?;

                    mci.create_response(
                        ctx.serenity_context(),
                        poise::serenity_prelude::CreateInteractionResponse::UpdateMessage(
                            poise::serenity_prelude::CreateInteractionResponseMessage::default()
                                .content("")
                                .components(vec![])
                                .embeds(vec![
                                    CreateEmbed::default()
                                        .title("✅ Book Removed")
                                        .description(format!(
                                            "Removed '{}' from the queue.",
                                            book_title
                                        ))
                                        .color(0xB76E79)
                                        .footer(CreateEmbedFooter::new(
                                            "Powered by Google Books API",
                                        )),
                                ]),
                        ),
                    )
                    .await
                    .ok();
                } else {
                    mci.create_response(
                        ctx.serenity_context(),
                        poise::serenity_prelude::CreateInteractionResponse::UpdateMessage(
                            poise::serenity_prelude::CreateInteractionResponseMessage::default()
                                .content("")
                                .components(vec![])
                                .embeds(vec![
                                    CreateEmbed::default()
                                        .title("❌ Cancelled")
                                        .description("Book removal cancelled.")
                                        .color(0xB76E79),
                                ]),
                        ),
                    )
                    .await
                    .ok();
                }
            } else {
                msg.edit(
                    ctx.serenity_context(),
                    poise::serenity_prelude::EditMessage::default()
                        .content("")
                        .components(vec![])
                        .embeds(vec![
                            CreateEmbed::default()
                                .title("⏰ Timed Out")
                                .description("Book was not removed.")
                                .color(0xB76E79),
                        ]),
                )
                .await
                .ok();
            }
        }
        None => {
            let embed = CreateEmbed::default()
                .title("❌ Not in Queue")
                .description("This book is not in the queue.")
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }

    Ok(())
}

#[poise::command(
    slash_command,
    rename = "user",
    description_localized("en-US", "Remove all books suggested by a specific user"),
    user_cooldown = 10
)]
async fn user(
    ctx: Context<'_>,
    #[description = "User whose books to remove"] user: User,
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

    // Get ALL books in the queue suggested by this user (for this server)
    let user_books = sqlx::query!(
        r#"
        SELECT sbq.volume_id, sbq.position
        FROM server_book_queue sbq
        WHERE sbq.server_id = $1 AND sbq.suggested_by_user_id = $2
        ORDER BY sbq.position
        "#,
        guild_id.get() as i64,
        user.id.get() as i64
    )
    .fetch_all(pool)
    .await?;

    if user_books.is_empty() {
        let embed = CreateEmbed::default()
            .title("❌ No Books in Queue")
            .description(format!(
                "{} doesn't have any books in the queue.",
                user.name
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Build a preview list of titles (up to 10) for the confirmation embed
    let preview_limit = 10usize;
    let total_count = user_books.len();
    let mut lines: Vec<String> = Vec::new();

    for (idx, b) in user_books.iter().enumerate() {
        if idx >= preview_limit {
            break;
        }
        // Try to enrich with title/authors; gracefully fall back to volume_id
        let (title, authors) = match google_books.get_volume(&b.volume_id).await {
            Ok(volume) => (volume.get_title(), volume.get_authors_string()),
            Err(_) => (
                format!("Book ({})", b.volume_id),
                "Unknown Author".to_string(),
            ),
        };
        lines.push(format!("#{} — {} — {}", b.position, title, authors));
    }

    if total_count > preview_limit {
        lines.push(format!("…and **{} more**", total_count - preview_limit));
    }

    let list_block = if lines.is_empty() {
        "No preview available.".to_string()
    } else {
        lines.join("\n")
    };

    // Confirmation embed clearly states it will remove ALL books by this user
    let embed = CreateEmbed::default()
        .title("🗑️ Remove all books by user?")
        .description(format!(
            "This will remove **{}** book(s) from the queue suggested by **{}**.",
            total_count, user.name
        ))
        .field("Books to be removed (preview)", list_block, false)
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Powered by Google Books API"));

    let components = vec![CreateActionRow::Buttons(vec![
        CreateButton::new("confirm_remove")
            .label("Confirm")
            .style(ButtonStyle::Danger),
        CreateButton::new("cancel")
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ])];

    let msg = ctx
        .send(
            poise::CreateReply::default()
                .embed(embed)
                .components(components),
        )
        .await?
        .into_message()
        .await?;

    // Wait for confirmation/cancel from the same user who invoked the command
    let mut stream = msg
        .await_component_interactions(ctx)
        .timeout(Duration::from_secs(60))
        .author_id(ctx.author().id)
        .stream();
    {
        while let Some(mci) = stream.next().await {
            if mci.data.custom_id == "confirm_remove" {
                // Delete ALL books suggested by that user for this server
                sqlx::query!(
                    "DELETE FROM server_book_queue 
                     WHERE server_id = $1 AND suggested_by_user_id = $2",
                    guild_id.get() as i64,
                    user.id.get() as i64
                )
                .execute(pool)
                .await?;

                // Build a concise post-action message
                let confirm_embed = CreateEmbed::default()
                    .title("✅ Books Removed")
                    .description(format!(
                        "Removed **{}** book(s) suggested by **{}** from the queue.",
                        total_count, user.name
                    ))
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Powered by Google Books API"));

                mci.create_response(
                    ctx.serenity_context(),
                    poise::serenity_prelude::CreateInteractionResponse::UpdateMessage(
                        poise::serenity_prelude::CreateInteractionResponseMessage::default()
                            .content("")
                            .components(vec![])
                            .embeds(vec![confirm_embed]),
                    ),
                )
                .await
                .ok();
            } else {
                mci.create_response(
                    ctx.serenity_context(),
                    poise::serenity_prelude::CreateInteractionResponse::UpdateMessage(
                        poise::serenity_prelude::CreateInteractionResponseMessage::default()
                            .content("❎ Cancelled.")
                            .components(vec![])
                            .embeds(vec![]),
                    ),
                )
                .await
                .ok();
            }
        }
    }

    Ok(())
}
