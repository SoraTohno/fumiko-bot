use crate::maturity_check::{
    check_volume_maturity, create_mature_content_warning, current_channel_is_nsfw,
    server_maturity_enabled,
};
use crate::types::QueryMode;
use crate::util::{
    detect_query_mode, embed_author_with_icon, get_guild_icon_url, get_guild_name, normalize_isbn,
    queue_commands_enabled,
};
use crate::*;
use crate::{types::Context, types::Error};
use poise::futures_util::StreamExt;
use poise::serenity_prelude::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage,
};
use std::time::Duration;

fn queue_disabled_embed() -> CreateEmbed {
    CreateEmbed::default()
        .title("🚫 Queue Command Disabled")
        .description(
            "Queue commands are disabled by a server admin. Ask them to use `/config queue enable` or use `/adminqueue` if you have permission.",
        )
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Configured via /config queue"))
}

#[poise::command(
    slash_command,
    subcommands("view", "add", "remove"),
    guild_only,
    description_localized("en-US", "Manage the book queue for this server"),
    user_cooldown = 10
)]
pub async fn queue(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "view",
    description_localized("en-US", "View the current book queue"),
    user_cooldown = 10
)]
async fn view(ctx: Context<'_>) -> Result<(), Error> {
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

    let queue = sqlx::query!(
        r#"
        SELECT
            sbq.position,
            sbq.volume_id,
            du.username as suggested_by,
            sbq.added_at
        FROM server_book_queue sbq
        JOIN discord_users du ON du.user_id = sbq.suggested_by_user_id
        WHERE sbq.server_id = $1
        ORDER BY sbq.position
        "#,
        guild_id.get() as i64
    )
    .fetch_all(pool)
    .await?;

    if queue.is_empty() {
        let embed = CreateEmbed::default()
            .author(embed_author_with_icon(
                format!("{} Book Queue", guild_name),
                guild_icon.clone(),
            ))
            .description("The queue is empty! Add books with `/queue add`.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));

        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Fetch book details from Google Books API and check maturity
    let mut filtered_queue = Vec::new();
    let mut mature_count = 0;
    let is_nsfw = current_channel_is_nsfw(&ctx).await?;
    let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;

    for book in queue.iter().take(10) {
        match google_books.get_volume(&book.volume_id).await {
            Ok(volume) => {
                if check_volume_maturity(&ctx, pool, &volume).await? {
                    filtered_queue.push((book, Some(volume)));
                } else {
                    mature_count += 1;
                }
            }
            Err(_) => {
                // Include books that fail to fetch (API error)
                filtered_queue.push((book, None));
            }
        }
    }

    if filtered_queue.is_empty() && mature_count > 0 {
        let embed = CreateEmbed::default()
            .author(embed_author_with_icon(
                format!("{} Book Queue", guild_name),
                guild_icon.clone(),
            ))
            .description(format!(
                "The queue contains {} book(s), but all are marked as mature content.\n\n\
                 To view mature books, an administrator must enable mature content with `/config mature enable` \
                 and this command must be used in an NSFW channel.",
                queue.len()
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Content rating from Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let mut embed = CreateEmbed::default()
        .author(embed_author_with_icon(
            format!("{} Book Queue", guild_name),
            guild_icon,
        ))
        .description(format!(
            "{} books in queue{}",
            queue.len(),
            if mature_count > 0 {
                let mut why: Vec<&str> = Vec::new();
                if !is_nsfw {
                    why.push("blocked in non-NSFW channel");
                }
                if !maturity_enabled {
                    why.push("maturity disabled");
                }
                if why.is_empty() {
                    format!(" ({} mature books hidden)", mature_count)
                } else {
                    format!(
                        " ({} mature books hidden — {})",
                        mature_count,
                        why.join(" & ")
                    )
                }
            } else {
                String::new()
            }
        ))
        .color(0xB76E79);

    for (book, volume_opt) in filtered_queue {
        match volume_opt {
            Some(volume) => {
                let title = volume.get_title();
                let authors = volume.get_authors_string();
                embed = embed.field(
                    format!("{}. {}", book.position, title),
                    format!("by {}\nSuggested by: {}", authors, book.suggested_by),
                    false,
                );
            }
            None => {
                // Fallback if API fails
                embed = embed.field(
                    format!("{}. [Book data unavailable]", book.position),
                    format!(
                        "Volume ID: {}\nSuggested by: {}",
                        book.volume_id, book.suggested_by
                    ),
                    false,
                );
            }
        }
    }

    if queue.len() > 10 {
        embed = embed.footer(CreateEmbedFooter::new(format!(
            "Showing first 10 of {} books • Powered by Google Books API",
            queue.len()
        )));
    } else {
        embed = embed.footer(CreateEmbedFooter::new("Powered by Google Books API"));
    }

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Add a book to the queue. One per user."),
    user_cooldown = 10,
    guild_only
)]
async fn add(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13"] title_or_isbn: String,
    #[description = "Author name (optional; used when title)"] author: Option<String>,
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

    let pool = &ctx.data().database;

    if !queue_commands_enabled(pool, guild_id.get() as i64).await? {
        ctx.send(poise::CreateReply::default().embed(queue_disabled_embed()))
            .await?;
        return Ok(());
    }

    let guild_name = if let Some(g) = ctx.guild() {
        g.name.clone()
    } else {
        match guild_id
            .to_partial_guild(ctx.serenity_context().http.clone())
            .await
        {
            Ok(pg) => pg.name,
            Err(_) => format!("Server {}", guild_id.get()),
        }
    };

    let google_books = &ctx.data().google_books;

    ensure_user_exists(pool, ctx.author()).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    // Search for the book using unified logic
    let chosen = detect_query_mode(&title_or_isbn);

    let mut result_bool: bool = false;

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

            if results.len() > 1 {
                result_bool = true;
            }

            let selected = results.into_iter().next().unwrap();

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

    let user_book = sqlx::query!(
        "SELECT volume_id FROM server_book_queue 
         WHERE server_id = $1 AND suggested_by_user_id = $2",
        guild_id.get() as i64,
        ctx.author().id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    // check if user already has a book in the queue
    if user_book.is_some() {
        let embed = CreateEmbed::default()
            .title("❌ Queue Limit")
            .description("You already have a book in the queue! Use `/queue remove` first to change your suggestion.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));

        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let volume_id = &book.id;
    let book_title = book.get_title();
    let book_authors = book.get_authors_string();

    // Check if book is already in queue
    let already_queued = sqlx::query!(
        "SELECT suggested_by_user_id FROM server_book_queue WHERE server_id = $1 AND volume_id = $2",
        guild_id.get() as i64,
        volume_id
    )
    .fetch_optional(pool)
    .await?;

    if let Some(existing_suggestion) = already_queued {
        let suggester = sqlx::query!(
            "SELECT username FROM discord_users WHERE user_id = $1",
            existing_suggestion.suggested_by_user_id
        )
        .fetch_one(pool)
        .await?;

        let embed = CreateEmbed::default()
            .title("Already in Queue")
            .description(format!(
                "This book is already in the queue (suggested by {})!",
                suggester.username
            ))
            .field("Title", &book_title, false)
            .field("Authors", &book_authors, false)
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));

        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Add to queue
    let result = sqlx::query!(
        "INSERT INTO server_book_queue (server_id, volume_id, suggested_by_user_id, position)
        VALUES ($1, $2, $3, (
            SELECT COALESCE(MAX(position), 0) + 1
            FROM server_book_queue
            WHERE server_id = $1
        ))
        RETURNING position",
        guild_id.get() as i64,
        volume_id,
        ctx.author().id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    match result {
        Some(record) => {
            let mut embed = CreateEmbed::default()
                .title("✅ Book Added to Queue")
                .field("Title", &book_title, false)
                .field("Authors", &book_authors, false)
                .field("Position", format!("#{}", record.position), true)
                .color(0xB76E79);

            if let Some(thumbnail_url) = book.get_thumbnail_url() {
                embed = embed.thumbnail(thumbnail_url);
            }

            // Check for prior ratings of this book in this server
            let completed_book = sqlx::query!(
                r#"
                SELECT completed_id, average_rating, total_ratings
                FROM server_completed_books
                WHERE server_id = $1 AND volume_id = $2
                ORDER BY completed_at DESC
                LIMIT 1
                "#,
                guild_id.get() as i64,
                volume_id
            )
            .fetch_optional(pool)
            .await?;

            if let Some(book_info) = completed_book {
                if let Some(avg) = book_info.average_rating {
                    embed = embed.field(
                        "Previous Rating",
                        format!(
                            "{:.1}/5 ({} ratings)",
                            avg,
                            book_info.total_ratings.unwrap_or(0)
                        ),
                        false,
                    );
                }
            }

            let footer_text = if result_bool {
                "Multiple books found. • Powered by Google Books API"
            } else {
                "Powered by Google Books API"
            };
            embed = embed.footer(CreateEmbedFooter::new(footer_text));

            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
        None => {
            let embed = CreateEmbed::default()
                .title("❌ Queue Limit")
                .description("You already have a book in the queue! Use `/queue remove` first to change your suggestion.")
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));

            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }

    Ok(())
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Remove your book from the queue"),
    user_cooldown = 10
)]
async fn remove(ctx: Context<'_>) -> Result<(), Error> {
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

    if !queue_commands_enabled(pool, guild_id.get() as i64).await? {
        ctx.send(poise::CreateReply::default().embed(queue_disabled_embed()))
            .await?;
        return Ok(());
    }

    let user_books = sqlx::query!(
        r#"
        SELECT sbq.volume_id, sbq.position
        FROM server_book_queue sbq
        WHERE sbq.server_id = $1 AND sbq.suggested_by_user_id = $2
        ORDER BY sbq.position
        "#,
        guild_id.get() as i64,
        ctx.author().id.get() as i64
    )
    .fetch_all(pool)
    .await?;

    if user_books.is_empty() {
        let embed = CreateEmbed::default()
            .title("No Book in Queue")
            .description("You don't have a book in the queue.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));

        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let total_count = user_books.len();
    let preview_limit = 10usize;
    let mut lines: Vec<String> = Vec::new();
    let mut first_title_for_confirm: Option<String> = None;

    for (idx, book) in user_books.iter().enumerate() {
        if idx >= preview_limit {
            break;
        }

        let (title, authors) = match google_books.get_volume(&book.volume_id).await {
            Ok(volume) => (volume.get_title(), volume.get_authors_string()),
            Err(_) => (
                format!("Book ({})", book.volume_id),
                "Unknown Author".to_string(),
            ),
        };

        if first_title_for_confirm.is_none() {
            first_title_for_confirm = Some(title.clone());
        }

        lines.push(format!("#{} — {} — {}", book.position, title, authors));
    }

    if total_count > preview_limit {
        lines.push(format!("…and **{} more**", total_count - preview_limit));
    }

    let list_block = if lines.is_empty() {
        "No preview available.".to_string()
    } else {
        lines.join("\n")
    };

    let embed = CreateEmbed::default()
        .title("🗑️ Remove all your books?")
        .description(format!(
            "This will remove **{}** book(s) you have suggested from the queue.",
            total_count
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

    let message = ctx
        .send(
            poise::CreateReply::default()
                .embed(embed)
                .components(components),
        )
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
            sqlx::query!(
                "DELETE FROM server_book_queue
                 WHERE server_id = $1 AND suggested_by_user_id = $2",
                guild_id.get() as i64,
                ctx.author().id.get() as i64
            )
            .execute(pool)
            .await?;

            let confirm_embed = if total_count == 1 {
                let fallback = format!("Book ({})", user_books[0].volume_id);
                let title = first_title_for_confirm.unwrap_or(fallback);

                CreateEmbed::default()
                    .title("✅ Book Removed")
                    .description(format!("Removed '{}' from the queue.", title))
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Powered by Google Books API"))
            } else {
                CreateEmbed::default()
                    .title("✅ Books Removed")
                    .description(format!(
                        "Removed **{}** book(s) you suggested from the queue.",
                        total_count
                    ))
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Powered by Google Books API"))
            };

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
