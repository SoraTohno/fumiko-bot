use crate::maturity_check::{
    check_volume_maturity, create_mature_content_warning, current_channel_is_nsfw,
    server_maturity_enabled,
};
use crate::types::QueryMode;
use crate::util::{
    detect_query_mode, embed_author_with_icon, get_guild_name, log_error_with_source,
    normalize_isbn,
};
use crate::*;
use crate::{types::Context, types::Error};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, User};

#[poise::command(
    slash_command,
    subcommands("view", "add", "remove"),
    guild_only,
    description_localized("en-US", "Manage your reading list for this server"),
    user_cooldown = 10
)]
pub async fn readinglist(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "view",
    guild_only,
    description_localized("en-US", "View a reading list in this server"),
    user_cooldown = 10
)]
async fn view(
    ctx: Context<'_>,
    #[description = "User to check reading list for (defaults to you)"] user: Option<User>,
) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let google_books = &ctx.data().google_books;
    let target_user = user.as_ref().unwrap_or_else(|| ctx.author());
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };
    let guild_name = get_guild_name(&ctx).await;

    let reading_list = sqlx::query!(
        r#"
        SELECT 
            volume_id,
            added_at
        FROM user_reading_list
        WHERE user_id = $1 AND server_id = $2
        ORDER BY added_at DESC
        LIMIT 5
        "#,
        target_user.id.get() as i64,
        guild_id.get() as i64
    )
    .fetch_all(pool)
    .await?;

    let footer_base = format!("{} • Max: 5 per server", guild_name);

    if reading_list.is_empty() {
        let msg = if target_user.id == ctx.author().id {
            "Your reading list in this server is empty! Add books with `/readinglist add`."
        } else {
            &format!(
                "{}'s reading list in this server is empty.",
                target_user.name
            )
        };

        let embed = CreateEmbed::default()
            .author(embed_author_with_icon(
                format!("{}'s Reading List", target_user.name),
                Some(target_user.face()),
            ))
            .description(msg)
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new(format!(
                "{} • Powered by Google Books API",
                footer_base
            )));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Fetch book details from Google Books
    let volume_ids: Vec<String> = reading_list.iter().map(|r| r.volume_id.clone()).collect();
    let volumes = google_books.get_volumes_batch(&volume_ids).await;

    // Filter out mature books if they can't be displayed
    let mut filtered_list = Vec::new();
    let mut mature_count = 0;
    let is_nsfw = current_channel_is_nsfw(&ctx).await?;
    let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;

    for (i, book) in reading_list.iter().enumerate() {
        match volumes.get(i) {
            Some(Ok(volume)) => {
                if check_volume_maturity(&ctx, pool, volume).await? {
                    filtered_list.push((book, Some(volume.clone())));
                } else {
                    mature_count += 1;
                }
            }
            _ => {
                // Include books that fail to fetch (API error)
                filtered_list.push((book, None));
            }
        }
    }

    if filtered_list.is_empty() && mature_count > 0 {
        let embed = CreateEmbed::default()
            .author(embed_author_with_icon(
                format!("{}'s Reading List", target_user.name),
                Some(target_user.face()),
            ))
            .description(format!(
                "All of {}'s reading list entries in this server are marked as mature content.\n\n\
                 To view mature books, an administrator must enable mature content with `/config mature enable` \
                 and this command must be used in an NSFW channel.",
                target_user.name
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new(format!(
                "{} • Content rating from Google Books API",
                footer_base
            )));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let mut embed = CreateEmbed::default()
        .author(embed_author_with_icon(
            format!("{}'s Reading List", target_user.name),
            Some(target_user.face()),
        ))
        .color(0xB76E79);

    if mature_count > 0 {
        let mut tips: Vec<&str> = Vec::new();
        if !maturity_enabled {
            tips.push("enable mature content with `/config mature enable`");
        }
        if !is_nsfw {
            tips.push("use this command in an NSFW channel");
        }

        let mut message = String::from("Some mature books are hidden from this list.");
        if !tips.is_empty() {
            message.push(' ');
            message.push_str("To view them, ");
            message.push_str(&tips.join(" and "));
            message.push('.');
        }

        embed = embed.description(message);
    }

    for (i, (book, volume_opt)) in filtered_list.iter().enumerate() {
        match volume_opt {
            Some(volume) => {
                let title = volume.get_title();
                let authors = volume.get_authors_string();
                embed = embed.field(
                    format!("{}. {}", i + 1, title),
                    format!("by {}", authors),
                    false,
                );
            }
            None => {
                // Fallback if API fails
                embed = embed.field(
                    format!("{}. [Book data unavailable]", i + 1),
                    format!("Volume ID: {}", book.volume_id),
                    false,
                );
            }
        }
    }

    embed = embed.footer(CreateEmbedFooter::new(format!(
        "{} • Powered by Google Books API",
        footer_base
    )));
    ctx.send(poise::CreateReply::default().embed(embed)).await?;

    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "Add a book to your reading list in this server"),
    user_cooldown = 10
)]
async fn add(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13"] title_or_isbn: String,
    #[description = "Author name (optional; used when title)"] author: Option<String>,
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
    let guild_name = get_guild_name(&ctx).await;

    ensure_user_exists(pool, ctx.author()).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    // Check current count for this server
    let count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM user_reading_list WHERE user_id = $1 AND server_id = $2",
        ctx.author().id.get() as i64,
        guild_id.get() as i64
    )
    .fetch_one(pool)
    .await?
    .unwrap_or(0);

    if count >= 5 {
        let embed = CreateEmbed::default()
            .title("❌ Reading List Full")
            .description("You've reached the maximum of 5 books in your reading list for this server. Please remove some books before adding more.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Search for the book
    let chosen = detect_query_mode(&title_or_isbn);
    let mut result_bool = false;

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

            if results.len() > 1 {
                // Will be shown in footer of success/already exists embed
                result_bool = true;
            }

            let selected = results.into_iter().next().unwrap();

            selected
        }
    };

    if !check_volume_maturity(&ctx, pool, &book).await? {
        let is_nsfw = current_channel_is_nsfw(&ctx).await?;
        let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;
        let embed =
            create_mature_content_warning(Some(&book.get_title()), is_nsfw, maturity_enabled);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let book_title = book.get_title();
    let book_authors = book.get_authors_string();

    let result = sqlx::query!(
        "INSERT INTO user_reading_list (user_id, server_id, volume_id)
         VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
        ctx.author().id.get() as i64,
        guild_id.get() as i64,
        &book.id
    )
    .execute(pool)
    .await;

    match result {
        Ok(res) => {
            if res.rows_affected() > 0 {
                let mut embed = CreateEmbed::default()
                    .title("✅ Book Added to Reading List")
                    .field("Title", &book_title, false)
                    .field("Authors", &book_authors, false)
                    .field("Server", guild_name.clone(), true)
                    .field("Reading List Count", format!("{}/5", count + 1), true)
                    .color(0xB76E79);

                let footer_text = if result_bool {
                    "Multiple books found. • Powered by Google Books API"
                } else {
                    "Powered by Google Books API"
                };
                embed = embed.footer(CreateEmbedFooter::new(footer_text));

                ctx.send(poise::CreateReply::default().embed(embed)).await?;
            } else {
                let mut embed = CreateEmbed::default()
                    .title("Already in Reading List")
                    .description(format!(
                        "*{}* by {} is already in your reading list for this server!",
                        book_title, book_authors
                    ))
                    .color(0xB76E79);

                let footer_text = if result_bool {
                    "Multiple books found • Powered by Google Books API"
                } else {
                    "Powered by Google Books API"
                };
                embed = embed.footer(CreateEmbedFooter::new(footer_text));
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
            }
        }
        Err(e) => {
            log_error_with_source("Error adding to reading list", &e);
            let embed = CreateEmbed::default()
                .title("❌ Error")
                .description("An error occurred while adding the book to your reading list.")
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }

    Ok(())
}

#[poise::command(
    slash_command,
    rename = "remove",
    guild_only,
    description_localized("en-US", "Remove a book from your reading list in this server"),
    user_cooldown = 10
)]
async fn remove(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13"] title_or_isbn: String,
    #[description = "Author name (optional; used when title)"] author: Option<String>,
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
    let guild_name = get_guild_name(&ctx).await;

    ensure_user_exists(pool, ctx.author()).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

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
            let mut results = google_books
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

            if results.len() > 1 {
                // Prefer one that is actually on the user's reading list in this server
                let user_id = ctx.author().id.get() as i64;
                let reading_ids: Vec<String> = sqlx::query!(
                    "SELECT volume_id FROM user_reading_list WHERE user_id = $1 AND server_id = $2",
                    user_id,
                    guild_id.get() as i64
                )
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(|r| r.volume_id)
                .collect();

                if let Some(b) = results.iter().find(|b| reading_ids.contains(&b.id)) {
                    b.clone()
                } else {
                    let embed = CreateEmbed::default()
                        .title("❌ Book Not in Reading List")
                        .description("Multiple books found, but none match a book on your reading list in this server. Try a more specific title or use an ISBN.")
                        .color(0xB76E79)
                        .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                    ctx.send(poise::CreateReply::default().embed(embed)).await?;
                    return Ok(());
                }
            } else {
                results.remove(0)
            }
        }
    };

    let result = sqlx::query!(
        "DELETE FROM user_reading_list 
         WHERE user_id = $1 AND server_id = $2 AND volume_id = $3",
        ctx.author().id.get() as i64,
        guild_id.get() as i64,
        &book.id
    )
    .execute(pool)
    .await?;

    if result.rows_affected() > 0 {
        let embed = CreateEmbed::default()
            .title("✅ Book Removed from Reading List")
            .field("Title", book.get_title(), false)
            .field("Authors", book.get_authors_string(), false)
            .field("Server", guild_name.clone(), true)
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    } else {
        let embed = CreateEmbed::default()
            .title("❌ Book Not in List")
            .description("This book was not in your reading list for this server.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    }

    Ok(())
}
