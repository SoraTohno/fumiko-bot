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
    subcommands("add", "remove", "view"),
    guild_only,
    description_localized("en-US", "Manage your favorite books for this server"),
    user_cooldown = 10
)]
pub async fn favorite(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "Add a book to your favorites"),
    user_cooldown = 10
)]
async fn add(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13"] title_or_isbn: String,
    #[description = "The author for additional specification (optional, only used with title)"]
    author: Option<String>,
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

    ensure_user_exists(pool, &ctx.author()).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    // Check current count for this server
    let count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM user_favorite_books WHERE user_id = $1 AND server_id = $2",
        ctx.author().id.get() as i64,
        guild_id.get() as i64
    )
    .fetch_one(pool)
    .await?
    .unwrap_or(0);

    if count >= 5 {
        let embed = CreateEmbed::default()
            .title("❌ Favorites Limit Reached")
            .description("You've reached the maximum of 5 favorite books for this server. Please remove some favorites before adding more.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

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

    let result = sqlx::query!(
        "INSERT INTO user_favorite_books (user_id, server_id, volume_id)
        VALUES ($1, $2, $3)
        ON CONFLICT DO NOTHING",
        ctx.author().id.get() as i64,
        guild_id.get() as i64,
        volume_id
    )
    .execute(pool)
    .await;

    match result {
        Ok(res) => {
            if res.rows_affected() > 0 {
                let mut embed = CreateEmbed::default()
                    .title("✅ Book Added to Favorites")
                    .field("Title", &book_title, false)
                    .field("Authors", &book_authors, false)
                    .field("Server", guild_name.clone(), true)
                    .field("Favorites Count", format!("{}/5", count + 1), true)
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
                    .title("Already in Favorites")
                    .description(format!(
                        "*{}* by {} is already in your favorites for this server!",
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
            log_error_with_source("Error adding favorite", &e);
            let embed = CreateEmbed::default()
                .title("❌ Error")
                .description("An error occurred while adding the book to your favorites.")
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }

    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "Remove a book from your favorites"),
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
    ensure_user_exists(pool, &ctx.author()).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    let chosen = detect_query_mode(&title_or_isbn);

    // Resolve to a single book
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
                    .description("No books found.")
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Searched via Google Books API"));
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            if results.len() > 1 {
                // Narrow to one by checking which one is in the user's favorites for this server
                let user_id = ctx.author().id.get() as i64;
                let favorite_ids: Vec<String> = sqlx::query!(
                    "SELECT volume_id FROM user_favorite_books WHERE user_id = $1 AND server_id = $2",
                    user_id,
                    guild_id.get() as i64
                )
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(|r| r.volume_id)
                .collect();

                if let Some(b) = results.iter().find(|b| favorite_ids.contains(&b.id)) {
                    b.clone()
                } else {
                    let embed = CreateEmbed::default()
                        .title("❌ Book Not in Favorites")
                        .description("Multiple books found, but none are in your favorites for this server. Try searching by ISBN or using a more specific title.")
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

    // Remove from favorites for this server
    let result = sqlx::query!(
        "DELETE FROM user_favorite_books 
         WHERE user_id = $1 AND server_id = $2 AND volume_id = $3",
        ctx.author().id.get() as i64,
        guild_id.get() as i64,
        &book.id
    )
    .execute(pool)
    .await?;

    if result.rows_affected() > 0 {
        let embed = CreateEmbed::default()
            .title("✅ Book Removed from Favorites")
            .field("Title", book.get_title(), false)
            .field("Authors", book.get_authors_string(), false)
            .field("Server", guild_name.clone(), true)
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    } else {
        let embed = CreateEmbed::default()
            .title("❌ Not in Favorites")
            .description("This book was not in your favorites for this server.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    }

    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "View favorite books in this server"),
    user_cooldown = 10
)]
async fn view(
    ctx: Context<'_>,
    #[description = "User to check favorites for (defaults to you)"] user: Option<User>,
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
    let footer_base = format!("{} • Max: 5 per server", guild_name);
    let favorites = sqlx::query!(
        r#"
        SELECT 
            volume_id,
            added_at,
            is_number_one
        FROM user_favorite_books
        WHERE user_id = $1 AND server_id = $2
        ORDER BY is_number_one DESC, added_at DESC
        LIMIT 5
        "#,
        target_user.id.get() as i64,
        guild_id.get() as i64
    )
    .fetch_all(pool)
    .await?;

    if favorites.is_empty() {
        let msg = if target_user.id == ctx.author().id {
            "You don't have any favorite books in this server yet!"
        } else {
            &format!(
                "{} doesn't have any favorite books in this server yet!",
                target_user.name
            )
        };

        let embed = CreateEmbed::default()
            .author(embed_author_with_icon(
                format!("{}'s Favorite Books", target_user.name),
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
    let volume_ids: Vec<String> = favorites.iter().map(|f| f.volume_id.clone()).collect();
    let volumes = google_books.get_volumes_batch(&volume_ids).await;

    // Filter out mature books if they can't be displayed
    let mut filtered_favorites = Vec::new();
    let mut mature_count = 0;

    for (i, fav) in favorites.iter().enumerate() {
        match volumes.get(i) {
            Some(Ok(volume)) => {
                if check_volume_maturity(&ctx, pool, volume).await? {
                    filtered_favorites.push((fav, Some(volume.clone())));
                } else {
                    mature_count += 1;
                }
            }
            _ => {
                // Include books that fail to fetch (API error)
                filtered_favorites.push((fav, None));
            }
        }
    }

    let is_nsfw = current_channel_is_nsfw(&ctx).await?;
    let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;

    if filtered_favorites.is_empty() && mature_count > 0 {
        let embed = CreateEmbed::default()
            .author(embed_author_with_icon(
                format!("{}'s Favorite Books", target_user.name),
                Some(target_user.face()),
            ))
            .description(format!(
                "All of {}'s favorite books in this server are marked as mature content.\n\n\
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
            format!("{}'s Favorite Books", target_user.name),
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

    // Check if there's a #1 book and add its thumbnail
    let mut thumbnail_url: Option<String> = None;
    for (fav, volume_opt) in &filtered_favorites {
        if fav.is_number_one {
            if let Some(volume) = volume_opt {
                thumbnail_url = volume.get_thumbnail_url();
            }
            break;
        }
    }

    if let Some(url) = thumbnail_url {
        embed = embed.thumbnail(url);
    }

    // Add the book fields
    for (_i, (fav, volume_opt)) in filtered_favorites.iter().enumerate() {
        let bullet = if fav.is_number_one { "⭐" } else { "•" };

        match volume_opt {
            Some(volume) => {
                let title = volume.get_title();
                let authors = volume.get_authors_string();
                embed = embed.field(
                    format!("{} {}", bullet, title),
                    format!("by {}", authors),
                    false,
                );
            }
            None => {
                // Fallback if API fails
                embed = embed.field(
                    format!("{} [Book data unavailable]", bullet),
                    format!("Volume ID: {}", fav.volume_id),
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
