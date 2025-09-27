use crate::util::{detect_query_mode, normalize_isbn};
use crate::{types::Context, types::Error, types::QueryMode};
use poise::futures_util::StreamExt;
use poise::serenity_prelude::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter,
};
use sqlx::Row;
use std::time::Duration;

#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Remove a completed book from this server (requires Manage Server)",
    ),
    user_cooldown = 10
)]
pub async fn clubreadremove(
    ctx: Context<'_>,
    #[description = "Title, ISBN, or exact Google Books volume ID"] book: String,
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

    // First, try direct volume_id lookup for backwards compatibility
    let mut volume_ids_to_search = vec![book.clone()];

    // If the query doesn't look like a direct volume_id match, use Google Books API
    let direct_results = sqlx::query!(
        r#"
        SELECT completed_id FROM server_completed_books
        WHERE server_id = $1 AND volume_id = $2
        LIMIT 1
        "#,
        guild_id.get() as i64,
        book
    )
    .fetch_optional(pool)
    .await?;

    // If no direct match found, search via Google Books API
    if direct_results.is_none() {
        let search_mode = detect_query_mode(&book);

        match search_mode {
            QueryMode::Isbn => {
                let isbn = normalize_isbn(&book);
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

                match google_books.search_by_isbn(&isbn).await {
                    Ok(Some(volume)) => {
                        volume_ids_to_search = vec![volume.id];
                    }
                    Ok(None) => {
                        let embed = CreateEmbed::default()
                            .title("❌ Book Not Found")
                            .description(format!("No book found with ISBN: {}", book))
                            .color(0xB76E79)
                            .footer(CreateEmbedFooter::new("Searched via Google Books API"));
                        ctx.send(poise::CreateReply::default().embed(embed)).await?;
                        return Ok(());
                    }
                    Err(e) => {
                        let embed = CreateEmbed::default()
                            .title("❌ Search Error")
                            .description(format!("Error searching Google Books: {}", e))
                            .color(0xB76E79)
                            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                        ctx.send(poise::CreateReply::default().embed(embed)).await?;
                        return Ok(());
                    }
                }
            }
            QueryMode::Title => {
                match google_books
                    .search_books(&book, author.as_deref(), Some(10))
                    .await
                {
                    Ok(volumes) => {
                        if volumes.is_empty() {
                            let embed = CreateEmbed::default()
                                .title("❌ Book Not Found")
                                .description("No books found with that title.")
                                .color(0xB76E79)
                                .footer(CreateEmbedFooter::new("Searched via Google Books API"));
                            ctx.send(poise::CreateReply::default().embed(embed)).await?;
                            return Ok(());
                        }
                        volume_ids_to_search = volumes.into_iter().map(|v| v.id).collect();
                    }
                    Err(e) => {
                        let embed = CreateEmbed::default()
                            .title("❌ Search Error")
                            .description(format!("Error searching Google Books: {}", e))
                            .color(0xB76E79)
                            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                        ctx.send(poise::CreateReply::default().embed(embed)).await?;
                        return Ok(());
                    }
                }
            }
        }
    }

    // Now search for completed books using the volume_ids we found
    let placeholders: Vec<String> = (2..volume_ids_to_search.len() + 2)
        .map(|i| format!("${}", i))
        .collect();

    let query_str = if placeholders.is_empty() {
        format!(
            r#"
            SELECT
                completed_id,
                volume_id,
                completed_at
            FROM server_completed_books
            WHERE server_id = $1
              AND volume_id = $2
            ORDER BY completed_at DESC
            LIMIT 10
            "#
        )
    } else {
        format!(
            r#"
            SELECT
                completed_id,
                volume_id,
                completed_at
            FROM server_completed_books
            WHERE server_id = $1
              AND volume_id IN ($2{})
            ORDER BY completed_at DESC
            LIMIT 10
            "#,
            if placeholders.len() > 1 {
                format!(", {}", placeholders[1..].join(", "))
            } else {
                String::new()
            }
        )
    };

    let mut query = sqlx::query(&query_str).bind(guild_id.get() as i64);

    for volume_id in &volume_ids_to_search {
        query = query.bind(volume_id);
    }

    let rows = query.fetch_all(pool).await?;

    // Convert the results to the expected structure
    let book_rows: Vec<_> = rows
        .into_iter()
        .map(|row| BookRow {
            completed_id: row.get("completed_id"),
            volume_id: row.get("volume_id"),
            completed_at: row.get("completed_at"),
        })
        .collect();

    if book_rows.is_empty() {
        let embed = CreateEmbed::default()
            .title("❌ No Matches Found")
            .description("No finished books matched that query for this server.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Fetch book details from Google Books
    let volume_ids: Vec<String> = book_rows.iter().map(|b| b.volume_id.clone()).collect();
    let volumes = google_books.get_volumes_batch(&volume_ids).await;

    // If exactly one match, ask for a straightforward confirm
    if book_rows.len() == 1 {
        let b = &book_rows[0];

        // Get book details
        let (title, authors) = match volumes.first() {
            Some(Ok(volume)) => (volume.get_title(), volume.get_authors_string()),
            _ => (
                format!("Book ({})", b.volume_id),
                "Unknown Author".to_string(),
            ),
        };

        let when = b
            .completed_at
            .as_ref()
            .map(
                |t: &sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>| {
                    t.format("%B %d, %Y").to_string()
                },
            )
            .unwrap_or_else(|| "Unknown date".into());

        let confirm_id = format!("confirm_{}", b.completed_id);
        let cancel_id = "cancel".to_string();

        let embed = CreateEmbed::default()
            .title("🗑️ Remove finished book?")
            .description(format!(
                "*{}*\nby {}\nCompleted: {}\n\nThis will remove the book from history. Ratings and polls tied to it will also be deleted.",
                title,
                authors,
                when
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Click Confirm to remove, or Cancel • Book data from Google Books API"));

        let components = vec![CreateActionRow::Buttons(vec![
            CreateButton::new(confirm_id.clone())
                .label("Confirm")
                .style(ButtonStyle::Danger),
            CreateButton::new(cancel_id.clone())
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

        // Wait up to 120s for the invoker to click
        if let Some(mci) = msg
            .await_component_interactions(ctx.serenity_context())
            .author_id(ctx.author().id)
            .timeout(Duration::from_secs(120))
            .stream()
            .next()
            .await
        {
            let cid = mci.data.custom_id.as_str().to_string();
            if cid == confirm_id {
                // Delete (cascades will remove ratings and polls automatically)
                let deleted = sqlx::query!(
                    r#"
                    DELETE FROM server_completed_books
                    WHERE server_id = $1 AND completed_id = $2
                    RETURNING volume_id
                    "#,
                    guild_id.get() as i64,
                    b.completed_id
                )
                .fetch_optional(pool)
                .await?;

                let reply_text = if let Some(_d) = deleted {
                    format!("Removed '*{}*' from completed books.", title)
                } else {
                    "⚠️ That entry was already removed or not found.".to_string()
                };

                mci.create_response(
                    ctx.serenity_context(),
                    poise::serenity_prelude::CreateInteractionResponse::UpdateMessage(
                        poise::serenity_prelude::CreateInteractionResponseMessage::default()
                            .content("")
                            .components(vec![])
                            .embeds(vec![
                                CreateEmbed::default()
                                    .title("✅ Book Removed")
                                    .description(reply_text)
                                    .color(0xB76E79)
                                    .footer(CreateEmbedFooter::new("Powered by Google Books API")),
                            ]),
                    ),
                )
                .await
                .ok();
            } else {
                // Cancel
                mci.create_response(
                    ctx.serenity_context(),
                    poise::serenity_prelude::CreateInteractionResponse::UpdateMessage(
                        poise::serenity_prelude::CreateInteractionResponseMessage::default()
                            .content("")
                            .components(vec![])
                            .embeds(vec![
                                CreateEmbed::default()
                                    .title("⏹ Cancelled")
                                    .description("No changes made.")
                                    .color(0xB76E79),
                            ]),
                    ),
                )
                .await
                .ok();
            }
        } else {
            // Timeout
            msg.edit(
                ctx.serenity_context(),
                poise::serenity_prelude::EditMessage::default()
                    .content("")
                    .components(vec![])
                    .embeds(vec![
                        CreateEmbed::default()
                            .title("⏰ Timed Out")
                            .description("No changes made.")
                            .color(0xB76E79),
                    ]),
            )
            .await
            .ok();
        }

        return Ok(());
    }

    // Multiple matches: show a short list with numbered delete buttons
    let mut embed = CreateEmbed::default()
        .title("🗑️ Select a finished book to remove")
        .description("Up to 10 matches shown. Click a button to confirm removal.")
        .color(0xB76E79);

    for (idx, b) in book_rows.iter().enumerate() {
        let (title, authors) = match volumes.get(idx) {
            Some(Ok(volume)) => (volume.get_title(), volume.get_authors_string()),
            _ => (
                format!("Book ({})", b.volume_id),
                "Unknown Author".to_string(),
            ),
        };

        let when = b
            .completed_at
            .as_ref()
            .map(
                |t: &sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>| {
                    t.format("%B %d, %Y").to_string()
                },
            )
            .unwrap_or_else(|| "Unknown date".into());

        embed = embed.field(
            format!("{}. {}", idx + 1, title),
            format!(
                "by {}\nVolume ID: {}\nCompleted: {}",
                authors, b.volume_id, when
            ),
            false,
        );
    }
    embed = embed.footer(CreateEmbedFooter::new(
        "Ratings and polls for a removed book are deleted as well • Powered by Google Books API",
    ));

    // Make up to two rows of 5 buttons (Discord limit = 5 per row)
    let mut buttons: Vec<CreateButton> = Vec::new();
    for (idx, b) in book_rows.iter().enumerate() {
        let cid = format!("rm_{}", b.completed_id);
        buttons.push(
            CreateButton::new(cid)
                .label(format!("Remove #{}", idx + 1))
                .style(ButtonStyle::Danger),
        );
    }
    let cancel_id = "cancel".to_string();

    let mut action_rows: Vec<CreateActionRow> = Vec::new();
    for chunk in buttons.chunks(5) {
        action_rows.push(CreateActionRow::Buttons(chunk.to_vec()));
    }
    action_rows.push(CreateActionRow::Buttons(vec![
        CreateButton::new(cancel_id.clone())
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ]));

    let mut msg = ctx
        .send(
            poise::CreateReply::default()
                .embed(embed)
                .components(action_rows.clone()),
        )
        .await?
        .into_message()
        .await?;

    // Collect only the invoker's clicks; stop after 120s
    let collector = msg
        .await_component_interactions(ctx.serenity_context())
        .author_id(ctx.author().id)
        .timeout(Duration::from_secs(120));

    let mut stream = collector.stream();
    while let Some(mci) = stream.next().await {
        let cid = mci.data.custom_id.as_str().to_string();
        if cid == cancel_id {
            mci.create_response(
                ctx.serenity_context(),
                poise::serenity_prelude::CreateInteractionResponse::UpdateMessage(
                    poise::serenity_prelude::CreateInteractionResponseMessage::default()
                        .content("")
                        .components(vec![])
                        .embeds(vec![
                            CreateEmbed::default()
                                .title("⏹ Cancelled")
                                .description("No changes made.")
                                .color(0xB76E79),
                        ]),
                ),
            )
            .await
            .ok();
            break;
        }

        // If a "rm_<completed_id>" button was pressed:
        if let Some(id_str) = cid.strip_prefix("rm_") {
            if let Ok(completed_id) = id_str.parse::<i32>() {
                // Find the book title for confirmation message
                let book_info = book_rows
                    .iter()
                    .enumerate()
                    .find(|(_, b)| b.completed_id == completed_id)
                    .and_then(|(idx, _)| volumes.get(idx));

                let title = match book_info {
                    Some(Ok(volume)) => volume.get_title(),
                    _ => "the book".to_string(),
                };

                let deleted = sqlx::query!(
                    r#"
                    DELETE FROM server_completed_books
                    WHERE server_id = $1 AND completed_id = $2
                    RETURNING volume_id
                    "#,
                    guild_id.get() as i64,
                    completed_id
                )
                .fetch_optional(pool)
                .await?;

                let reply_text = if deleted.is_some() {
                    format!("Removed '*{}*' from completed books.", title)
                } else {
                    "⚠️ That entry was already removed or not found.".to_string()
                };

                mci.create_response(
                    ctx.serenity_context(),
                    poise::serenity_prelude::CreateInteractionResponse::UpdateMessage(
                        poise::serenity_prelude::CreateInteractionResponseMessage::default()
                            .content("")
                            .components(vec![])
                            .embeds(vec![
                                CreateEmbed::default()
                                    .title("✅ Book Removed")
                                    .description(reply_text)
                                    .color(0xB76E79)
                                    .footer(CreateEmbedFooter::new("Powered by Google Books API")),
                            ]),
                    ),
                )
                .await
                .ok();

                break;
            }
        }
    }

    // If timed out with no selection, tidy the message
    msg.edit(
        ctx.serenity_context(),
        poise::serenity_prelude::EditMessage::default().components(vec![]),
    )
    .await
    .ok();

    Ok(())
}

// Helper struct to represent the query results
struct BookRow {
    completed_id: i32,
    volume_id: String,
    completed_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
}
