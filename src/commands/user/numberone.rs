use crate::types::QueryMode;
use crate::util::get_guild_name;
use crate::util::{detect_query_mode, normalize_isbn};
use crate::*;
use crate::{types::Context, types::Error};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter};

#[poise::command(
    slash_command,
    subcommands("set", "remove"),
    guild_only,
    description_localized("en-US", "Manage your #1 favorite book for this server"),
    user_cooldown = 10
)]
pub async fn numberone(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set #1 favorite book for this server
#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "Set your #1 favorite book for this server")
)]
async fn set(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13"] title_or_isbn: String,
    #[description = "Author (optional; used when searching by title)"] author: Option<String>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let pool = &ctx.data().database;
    let google = &ctx.data().google_books;
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

    let user_id = ctx.author().id.get() as i64;
    let server_id = guild_id.get() as i64;

    // Already have a #1 book for this server?
    if sqlx::query_scalar!(
        "SELECT volume_id FROM user_favorite_books WHERE user_id = $1 AND server_id = $2 AND is_number_one",
        user_id,
        server_id
    )
    .fetch_optional(pool)
    .await?
    .is_some() {
        let embed = CreateEmbed::default()
            .title("❌ Already Have #1 Book")
            .description("You already have a #1 favorite book for this server. Use `/numberone remove` first.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Ensure the user has at least one favorite book in this server
    let fav_count: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM user_favorite_books WHERE user_id = $1 AND server_id = $2",
        user_id,
        server_id
    )
    .fetch_one(pool)
    .await?
    .unwrap_or(0);

    if fav_count == 0 {
        let embed = CreateEmbed::default()
            .title("❌ No Favorite Books")
            .description("You don't have any favorite books in this server yet. Add one with `/favorite add` first.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Determine which book to set as #1
    let chosen = detect_query_mode(&title_or_isbn);

    // Capture the chosen volume ID and its title so doesn't re-hit Google later.
    let (target_volume_id, selected_title): (String, Option<String>) = match chosen {
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
            match google.search_by_isbn(&isbn).await? {
                Some(vol) => (vol.id.clone(), Some(vol.get_title())),
                None => {
                    let embed = CreateEmbed::default()
                        .title("❌ Book Not Found")
                        .description("No book found for that ISBN.")
                        .color(0xB76E79)
                        .footer(CreateEmbedFooter::new("Searched via Google Books API"));
                    ctx.send(poise::CreateReply::default().embed(embed)).await?;
                    return Ok(());
                }
            }
        }
        QueryMode::Title => {
            let mut results = google
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

            // Prefer one already in favorites for this server if possible
            let favorite_ids: Vec<String> = sqlx::query!(
                "SELECT volume_id FROM user_favorite_books WHERE user_id = $1 AND server_id = $2",
                user_id,
                server_id
            )
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| r.volume_id)
            .collect();

            results.sort_by_key(|v| {
                let id = v.id.clone();
                (!favorite_ids.contains(&id), id)
            });

            let chosen = results.remove(0);
            (chosen.id.clone(), Some(chosen.get_title()))
        }
    };

    // Ensure the chosen book is already in favorites for this server
    let affected = sqlx::query!(
        "UPDATE user_favorite_books
         SET is_number_one = TRUE
         WHERE user_id = $1 AND server_id = $2 AND volume_id = $3",
        user_id,
        server_id,
        target_volume_id
    )
    .execute(pool)
    .await?
    .rows_affected();

    if affected == 0 {
        // Use cached title if we have it; otherwise, best-effort fetch once.
        let book_title = if let Some(t) = selected_title.clone() {
            t
        } else {
            match google.get_volume(&target_volume_id).await {
                Ok(vol) => vol.get_title(),
                Err(_) => "That book".to_string(),
            }
        };

        let embed = CreateEmbed::default()
            .title("❌ Book Not in Favorites")
            .description(format!(
                "{} isn't in your favorites for this server. Add it with `/favorite add` first.",
                book_title
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Clear any other number_one flags for safety (for this server)
    sqlx::query!(
        "UPDATE user_favorite_books
         SET is_number_one = FALSE
         WHERE user_id = $1 AND server_id = $2 AND volume_id <> $3 AND is_number_one",
        user_id,
        server_id,
        target_volume_id
    )
    .execute(pool)
    .await?;

    // Confirmation: use cached title to avoid another API call
    let book_title = selected_title.unwrap_or_else(|| "Book".to_string());

    let embed = CreateEmbed::default()
        .title("⭐ #1 Favorite Book Set")
        .description(format!(
            "Set '{}' as your #1 favorite book in {}!",
            book_title, guild_name
        ))
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Powered by Google Books API"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Remove #1 favorite book for this server
#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "Remove your #1 favorite book for this server")
)]
async fn remove(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let pool = &ctx.data().database;
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };
    let guild_name = get_guild_name(&ctx).await;
    let user_id = ctx.author().id.get() as i64;
    let server_id = guild_id.get() as i64;

    let affected = sqlx::query!(
        "UPDATE user_favorite_books
         SET is_number_one = FALSE
         WHERE user_id = $1 AND server_id = $2 AND is_number_one",
        user_id,
        server_id
    )
    .execute(pool)
    .await?
    .rows_affected();

    if affected == 0 {
        let embed = CreateEmbed::default()
            .title("❌ No #1 Book Set")
            .description("You don't have a #1 favorite book set for this server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    } else {
        let embed = CreateEmbed::default()
            .title("✅ #1 Book Removed")
            .description(format!(
                "Removed your ⭐ #1 favorite book in {}.",
                guild_name
            ))
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    }
    Ok(())
}
