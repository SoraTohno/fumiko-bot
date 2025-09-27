use std::collections::HashMap;

use chrono::{Local, Utc};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter};

use crate::util::{embed_author_with_icon, get_guild_icon_url, get_guild_name};

use crate::{types::Context, types::Error};

fn format_timestamp(timestamp: Option<chrono::DateTime<Utc>>) -> String {
    timestamp
        .map(|dt| dt.with_timezone(&Local).format("%B %d, %Y").to_string())
        .unwrap_or_else(|| "Not available".to_string())
}

fn describe_rater(record: Option<(&str, i64, f64, i64)>, default: &str) -> String {
    match record {
        Some((username, user_id, average, count)) => {
            let plural = if count == 1 { "" } else { "s" };
            let mention_id: u64 = user_id as u64;
            format!(
                "<@{}> ({}) — {:.2}/5 across {} rating{}",
                mention_id, username, average, count, plural
            )
        }
        None => default.to_string(),
    }
}

#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "Show overall book club statistics for this server"),
    user_cooldown = 10
)]
pub async fn stats(ctx: Context<'_>) -> Result<(), Error> {
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

    let summary = sqlx::query!(
        r#"
        SELECT
            COUNT(*)::BIGINT         AS "total_books!",
            MIN(completed_at)        AS first_completed_at
        FROM server_completed_books
        WHERE server_id = $1
        "#,
        guild_id.get() as i64,
    )
    .fetch_one(pool)
    .await?;

    let server_install = sqlx::query!(
        r#"
        SELECT created_at
        FROM discord_servers
        WHERE server_id = $1
        "#,
        guild_id.get() as i64,
    )
    .fetch_optional(pool)
    .await?;

    let server_install_timestamp = server_install.and_then(|row| row.created_at);

    let rating_summary = sqlx::query!(
        r#"
        SELECT
            AVG(ubr.rating)::DOUBLE PRECISION AS average_rating,
            COUNT(*)::BIGINT                  AS "rating_count!"
        FROM user_book_ratings ubr
        JOIN server_completed_books scb ON scb.completed_id = ubr.completed_id
        WHERE scb.server_id = $1
        "#,
        guild_id.get() as i64,
    )
    .fetch_one(pool)
    .await?;

    let top_book = sqlx::query!(
        r#"
        SELECT
            volume_id          AS "volume_id!",
            average_rating     AS "average_rating!",
            total_ratings      AS "total_ratings!"
        FROM server_completed_books
        WHERE server_id = $1
          AND average_rating IS NOT NULL
          AND total_ratings > 0
        ORDER BY average_rating DESC, total_ratings DESC, completed_at DESC
        LIMIT 1
        "#,
        guild_id.get() as i64,
    )
    .fetch_optional(pool)
    .await?;

    let worst_book = sqlx::query!(
        r#"
        SELECT
            volume_id          AS "volume_id!",
            average_rating     AS "average_rating!",
            total_ratings      AS "total_ratings!"
        FROM server_completed_books
        WHERE server_id = $1
          AND average_rating IS NOT NULL
          AND total_ratings > 0
        ORDER BY average_rating ASC, total_ratings DESC, completed_at DESC
        LIMIT 1
        "#,
        guild_id.get() as i64,
    )
    .fetch_optional(pool)
    .await?;

    let highest_rater = sqlx::query!(
        r#"
        SELECT
            du.username      AS "username!",
            ubr.user_id      AS "user_id!",
            AVG(ubr.rating)::DOUBLE PRECISION AS "average_rating!",
            COUNT(*)::BIGINT AS "rating_count!"
        FROM user_book_ratings ubr
        JOIN server_completed_books scb ON scb.completed_id = ubr.completed_id
        JOIN discord_users du ON du.user_id = ubr.user_id
        WHERE scb.server_id = $1
        GROUP BY du.username, ubr.user_id
        HAVING COUNT(*) > 0
        ORDER BY AVG(ubr.rating) DESC, COUNT(*) DESC, du.username ASC
        LIMIT 1
        "#,
        guild_id.get() as i64,
    )
    .fetch_optional(pool)
    .await?;

    let lowest_rater = sqlx::query!(
        r#"
        SELECT
            du.username      AS "username!",
            ubr.user_id      AS "user_id!",
            AVG(ubr.rating)::DOUBLE PRECISION AS "average_rating!",
            COUNT(*)::BIGINT AS "rating_count!"
        FROM user_book_ratings ubr
        JOIN server_completed_books scb ON scb.completed_id = ubr.completed_id
        JOIN discord_users du ON du.user_id = ubr.user_id
        WHERE scb.server_id = $1
        GROUP BY du.username, ubr.user_id
        HAVING COUNT(*) > 0
        ORDER BY AVG(ubr.rating) ASC, COUNT(*) DESC, du.username ASC
        LIMIT 1
        "#,
        guild_id.get() as i64,
    )
    .fetch_optional(pool)
    .await?;

    let mut volume_ids = Vec::new();
    if let Some(book) = &top_book {
        volume_ids.push(book.volume_id.clone());
    }
    if let Some(book) = &worst_book {
        if !volume_ids.iter().any(|id| id == &book.volume_id) {
            volume_ids.push(book.volume_id.clone());
        }
    }

    let mut volumes: HashMap<String, crate::google_books::Volume> = HashMap::new();
    if !volume_ids.is_empty() {
        for (id, result) in volume_ids
            .iter()
            .zip(google_books.get_volumes_batch(&volume_ids).await)
        {
            if let Ok(volume) = result {
                volumes.insert(id.clone(), volume);
            }
        }
    }

    let most_liked_text = if let Some(book) = &top_book {
        let title = volumes
            .get(&book.volume_id)
            .map(|v| v.get_title())
            .unwrap_or_else(|| format!("Book ({})", book.volume_id));
        let author = volumes
            .get(&book.volume_id)
            .map(|v| v.get_authors_string())
            .unwrap_or_else(|| "Unknown author".to_string());
        let plural = if book.total_ratings == 1 { "" } else { "s" };
        format!(
            "**{}** by {} — {:.2}/5 from {} rating{}",
            title, author, book.average_rating, book.total_ratings, plural
        )
    } else {
        "No rated books yet.".to_string()
    };

    let most_disliked_text = if let Some(book) = &worst_book {
        let title = volumes
            .get(&book.volume_id)
            .map(|v| v.get_title())
            .unwrap_or_else(|| format!("Book ({})", book.volume_id));
        let author = volumes
            .get(&book.volume_id)
            .map(|v| v.get_authors_string())
            .unwrap_or_else(|| "Unknown author".to_string());
        let plural = if book.total_ratings == 1 { "" } else { "s" };
        format!(
            "**{}** by {} — {:.2}/5 from {} rating{}",
            title, author, book.average_rating, book.total_ratings, plural
        )
    } else {
        "No rated books yet.".to_string()
    };

    let average_rating_field = match rating_summary.average_rating {
        Some(avg) if rating_summary.rating_count > 0 => {
            let plural = if rating_summary.rating_count == 1 {
                ""
            } else {
                "s"
            };
            format!(
                "{:.2}/5 across {} rating{}",
                avg, rating_summary.rating_count, plural
            )
        }
        _ => "No ratings yet.".to_string(),
    };

    let embed = CreateEmbed::default()
        .author(embed_author_with_icon(
            format!("{} Book Club Stats", guild_name),
            guild_icon,
        ))
        .color(0xB76E79)
        .field("Books completed", summary.total_books.to_string(), true)
        .field("Average rating", average_rating_field, true)
        .field(
            "First book completed",
            if summary.total_books > 0 {
                format_timestamp(summary.first_completed_at)
            } else {
                "No books completed yet.".to_string()
            },
            true,
        )
        .field(
            "Bot installed",
            format_timestamp(server_install_timestamp),
            true,
        )
        .field("Most liked book", most_liked_text, false)
        .field("Most disliked book", most_disliked_text, false)
        .field(
            "Highest average rating",
            describe_rater(
                highest_rater.as_ref().map(|row| {
                    (
                        row.username.as_str(),
                        row.user_id,
                        row.average_rating,
                        row.rating_count,
                    )
                }),
                "No ratings yet.",
            ),
            false,
        )
        .field(
            "Lowest average rating",
            describe_rater(
                lowest_rater.as_ref().map(|row| {
                    (
                        row.username.as_str(),
                        row.user_id,
                        row.average_rating,
                        row.rating_count,
                    )
                }),
                "No ratings yet.",
            ),
            false,
        )
        .footer(CreateEmbedFooter::new(
            "Ratings sourced from members • Powered by Google Books API",
        ));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}
