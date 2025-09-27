use crate::maturity_check::{
    check_volume_maturity, create_mature_content_warning, current_channel_is_nsfw,
    server_maturity_enabled,
};
use crate::types::QueryMode;
use crate::util::{
    detect_query_mode, get_guild_name, is_valid_isbn10, is_valid_isbn13, normalize_isbn,
};
use crate::*;
use crate::{types::Context, types::Error};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, User};
use sqlx::types::BigDecimal;
use sqlx::types::chrono::{NaiveDate, TimeZone, Utc};
use std::str::FromStr;

#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Manually add a completed book to the server (requires Manage Server)",
    ),
    user_cooldown = 10
)]
pub async fn clubreadadd(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13"] title_or_isbn: String,
    #[description = "Author name (optional; helps find the right book)"] author: Option<String>,
    #[description = "Date completed (YYYY-MM-DD format)"] completion_date: Option<String>,
    #[description = "Average rating (1-5)"] rating: Option<f32>,
    #[description = "User who originally suggested this book"] suggested_by: Option<User>,
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

    ensure_server_exists(pool, guild_id, &guild_name).await?;

    // Validate rating if provided
    if let Some(r) = rating {
        if r < 1.0 || r > 5.0 {
            let embed = CreateEmbed::default()
                .title("❌ Invalid Rating")
                .description("Rating must be between 1 and 5 (inclusive).")
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    }

    // Parse completion date if provided
    let completed_at = if let Some(date_str) = completion_date.clone() {
        match NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
            Ok(date) => {
                // Reject future dates beyond tomorrow (compare by calendar date)
                let today = Utc::now().date_naive();
                let tomorrow = today.succ_opt().unwrap(); // safe for all valid dates

                if date > tomorrow {
                    let embed = CreateEmbed::default()
                        .title("❌ Invalid Completion Date")
                        .description("Completion date cannot be in the future beyond tomorrow.")
                        .color(0xB76E79);
                    ctx.send(poise::CreateReply::default().embed(embed)).await?;
                    return Ok(());
                }

                // Noon UTC to avoid timezone edge cases
                let datetime = date.and_hms_opt(12, 0, 0).unwrap();
                Some(Utc.from_utc_datetime(&datetime))
            }
            Err(_) => {
                let embed = CreateEmbed::default()
                    .title("❌ Invalid Date Format")
                    .description("Please use YYYY-MM-DD format (e.g., 2024-03-15).")
                    .color(0xB76E79);
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
                return Ok(());
            }
        }
    } else {
        None
    };

    // Ensure suggested_by user exists in database if provided
    if let Some(ref user) = suggested_by {
        ensure_user_exists(pool, user).await?;
    }

    // Search for the book
    let chosen = detect_query_mode(&title_or_isbn);
    let mut footer_notes: Vec<String> = Vec::new();

    let book = match chosen {
        QueryMode::Isbn => {
            let isbn = normalize_isbn(&title_or_isbn);
            if !is_plausible_isbn(&isbn) || !(is_valid_isbn10(&isbn) || is_valid_isbn13(&isbn)) {
                let embed = CreateEmbed::default()
                    .title("❌ Invalid ISBN")
                    .description(
                        "Please provide a valid ISBN-10 or ISBN-13 (with a correct checksum).",
                    )
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

            let selected = results[0].clone();

            if results.len() > 1 {
                footer_notes.push("Multiple books found.".to_string());
            }

            // let mut embed_alt = CreateEmbed::default()
            //     .title("Search Results")
            //     .description("Auto-selected the most relevant result below.")
            //     .color(0xB76E79);

            // for alt in results.iter().skip(1).take(2) {
            //     embed_alt = embed_alt.field(
            //         "Alternative",
            //         format!("**{}** • {}", alt.get_title(), alt.get_authors_string()),
            //         false,
            //     );
            // }

            // // Send a transient info message only if there were alternates
            // if results.len() > 1 {
            //     let _ = ctx
            //         .send(poise::CreateReply::default().embed(embed_alt))
            //         .await;
            // }

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

    // Only block exact same-day duplicates if a completion_date was provided.
    if completed_at.is_some() {
        let same_day_exists = sqlx::query_scalar!(
            r#"
            SELECT 1
            FROM server_completed_books
            WHERE server_id = $1
              AND volume_id = $2
              AND DATE(completed_at) = DATE($3)
            LIMIT 1
            "#,
            guild_id.get() as i64,
            volume_id,
            completed_at
        )
        .fetch_optional(pool)
        .await?;

        if same_day_exists.is_some() {
            let embed = CreateEmbed::default()
                .title("⚠️ Already Completed That Day")
                .description(format!(
                    "'{}' is already recorded as completed on {}.\n\nIf you intended a re-read, use a different completion date.",
                    book_title,
                    completed_at.unwrap().format("%B %d, %Y")
                ))
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    }

    // Calculate started_at (if completed_at is provided, assume it started 30 days before; otherwise use 30 days ago)
    let started_at = if let Some(completed) = completed_at {
        completed - chrono::Duration::days(30)
    } else {
        Utc::now() - chrono::Duration::days(30)
    };

    // Begin a transaction so the history insert and optional rating insert are atomic.
    let mut tx = pool.begin().await?;

    let rating_bd: Option<BigDecimal> = match rating {
        Some(r) => {
            let s = format!("{:.2}", r);
            BigDecimal::from_str(&s).ok()
        }
        None => None,
    };

    // Insert into server_completed_books
    // If completed_at is provided, write it; otherwise let it remain NULL
    let completed_id = if let Some(completed) = completed_at {
        sqlx::query_scalar!(
            r#"
            INSERT INTO server_completed_books 
                (server_id, volume_id, suggested_by_user_id, started_at, completed_at, average_rating, total_ratings)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING completed_id
            "#,
            guild_id.get() as i64,
            volume_id,
            suggested_by.as_ref().map(|u| u.id.get() as i64),
            started_at,
            completed,
            rating_bd,
            if rating.is_some() { 1 } else { 0 }
        )
        .fetch_one(&mut *tx)
        .await?
    } else {
        sqlx::query_scalar!(
            r#"
            INSERT INTO server_completed_books 
                (server_id, volume_id, suggested_by_user_id, started_at, average_rating, total_ratings)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING completed_id
            "#,
            guild_id.get() as i64,
            volume_id,
            suggested_by.as_ref().map(|u| u.id.get() as i64),
            started_at,
            rating_bd,
            if rating.is_some() { 1 } else { 0 }
        )
        .fetch_one(&mut *tx)
        .await?
    };

    // If a rating was provided and the admin wants to record it as their personal rating
    if let Some(r) = rating {
        // Guarantee the admin exists
        ensure_user_exists(pool, ctx.author()).await?;

        let rounded = r.round().clamp(1.0, 5.0) as i32;

        sqlx::query!(
            r#"
            INSERT INTO user_book_ratings (user_id, completed_id, rating)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id, completed_id) DO NOTHING
            "#,
            ctx.author().id.get() as i64,
            completed_id,
            rounded
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    // Build success embed
    let mut embed = CreateEmbed::default()
        .title("✅ Book Added to History")
        .field("Title", &book_title, false)
        .field("Authors", &book_authors, false);

    if let Some(user) = suggested_by {
        embed = embed.field("Suggested by", user.name, true);
    }

    if let Some(date) = completed_at {
        embed = embed.field("Completed", date.format("%B %d, %Y").to_string(), true);
    } else {
        embed = embed.field("Completed", "Today", true);
    }

    if let Some(r) = rating {
        embed = embed.field("Rating", format!("{:.1}/5", r), true);
    }

    embed = embed.color(0xB76E79);

    if let Some(thumbnail_url) = book.get_thumbnail_url() {
        embed = embed.thumbnail(thumbnail_url);
    }

    // Build footer
    let mut footer_text =
        String::from("Book manually added to history • Powered by Google Books API");
    if !footer_notes.is_empty() {
        footer_text.push_str(" • ");
        footer_text.push_str(&footer_notes.join(" • "));
    }
    embed = embed.footer(CreateEmbedFooter::new(footer_text));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/* ----------------------------- Helpers ----------------------------- */

fn is_plausible_isbn(isbn: &str) -> bool {
    // ISBN-10: digits + optional final 'X'
    // ISBN-13: digits only
    let len = isbn.len();
    if len == 10 {
        isbn.chars()
            .enumerate()
            .all(|(i, c)| c.is_ascii_digit() || (i == 9 && (c == 'X' || c == 'x')))
    } else if len == 13 {
        isbn.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}
