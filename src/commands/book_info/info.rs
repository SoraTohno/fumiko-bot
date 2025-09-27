use crate::google_books::Volume;
use crate::maturity_check::{
    check_volume_maturity, create_mature_content_warning, current_channel_is_nsfw,
    server_maturity_enabled,
};
use crate::types::{Context, Error, QueryMode};
use crate::util::{detect_query_mode, normalize_isbn, truncate_on_char_boundary};
use poise::serenity_prelude as serenity;
use poise::serenity_prelude::{CreateActionRow, CreateButton, CreateEmbed};
use std::time::Duration;

fn build_book_embed(
    book: &Volume,
    status: Option<&str>,
    favorites_count: i64,
    search_mode: Option<QueryMode>,
) -> CreateEmbed {
    let mut embed = CreateEmbed::default()
        .title(book.get_title())
        .color(0xB76E79);

    // Subtitle
    if let Some(subtitle) = &book.volume_info.subtitle {
        embed = embed.field("Subtitle", subtitle, false);
    }

    // Authors
    let authors = book.get_authors_string();
    embed = embed.field("Authors", authors.clone(), false);

    // Publisher / Date - only show for ISBN searches
    if matches!(search_mode, Some(QueryMode::Isbn)) {
        if let Some(publisher) = &book.volume_info.publisher {
            embed = embed.field("Publisher", publisher, true);
        }
        if let Some(date) = &book.volume_info.published_date {
            embed = embed.field("Published", date, true);
        }
    }

    // Pages
    // if let Some(pages) = book.get_page_count() {
    //     embed = embed.field("Pages", pages.to_string(), true);
    // }
    if let Some(pages) = book.get_page_count() {
        let pages_display = match search_mode {
            Some(QueryMode::Title) => {
                if let Some(publisher) = &book.volume_info.publisher {
                    format!("{} ({})", pages, publisher)
                } else {
                    pages.to_string()
                }
            }
            _ => pages.to_string(), // For ISBN searches or when mode is unknown
        };
        embed = embed.field("Pages", pages_display, true);
    }

    // Categories
    if let Some(categories) = &book.volume_info.categories {
        if !categories.is_empty() {
            let categories_str = categories
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            embed = embed.field("Categories", categories_str, false);
        }
    }

    // Description (trim for embed)
    if let Some(description) = book.get_description() {
        let desc = if description.len() > 500 {
            let (prefix, _) = truncate_on_char_boundary(&description, 497);
            format!("{prefix}...")
        } else {
            description
        };
        embed = embed.field("Description", desc, false);
    }

    // Status + Favorites
    embed = embed.field("Status", status.unwrap_or("Unknown"), true);
    if favorites_count > 0 {
        embed = embed.field(
            "Favorited by",
            format!("{} users in the server", favorites_count),
            true,
        );
    }

    // Useful Links
    let mut links: Vec<String> = Vec::new();
    if let Some(info_link) = &book.volume_info.info_link {
        links.push(format!("[Google Books]({})", info_link));
    } else {
        links.push(format!(
            "[Google Books](https://books.google.com/books?id={})",
            book.id
        ));
    }
    if let Some(preview_link) = &book.volume_info.preview_link {
        links.push(format!("[Preview]({})", preview_link));
    }
    embed = embed.field("Links", links.join(" • "), false);

    // Thumbnail
    if let Some(thumbnail_url) = book.get_thumbnail_url() {
        embed = embed.image(thumbnail_url);
    }

    // Google Books attribution
    embed = embed.footer(serenity::CreateEmbedFooter::new(
        "Powered by Google Books API",
    ));

    embed
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Get book info by title or ISBN"),
    user_cooldown = 5
)]
pub async fn book(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13"] title_or_isbn: String,
    #[description = "Author filter (for title searches)"] author: Option<String>,
) -> Result<(), Error> {
    ctx.defer().await?;

    let chosen = detect_query_mode(&title_or_isbn);
    let google = &ctx.data().google_books;
    let pool = &ctx.data().database;
    let guild_id = ctx.guild_id();

    match chosen {
        QueryMode::Isbn => {
            let isbn = normalize_isbn(&title_or_isbn);
            if isbn.len() != 10 && isbn.len() != 13 {
                ctx.say(format!(
                    "Invalid ISBN format. ISBN must be 10 or 13 characters long. You provided {} characters.",
                    isbn.len()
                )).await?;
                return Ok(());
            }

            match google.search_by_isbn(&isbn).await? {
                Some(book) => {
                    if !check_volume_maturity(&ctx, pool, &book).await? {
                        // Mature gate failed: tell the user why
                        let is_nsfw = current_channel_is_nsfw(&ctx).await?;
                        let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;
                        let embed = create_mature_content_warning(
                            Some(&book.get_title()),
                            is_nsfw,
                            maturity_enabled,
                        );
                        ctx.send(poise::CreateReply::default().embed(embed)).await?;
                        return Ok(());
                    }

                    let (status_display, favorites_count) =
                        if let Some(gid) = guild_id {
                            let volume_id = &book.id;
                            let status = sqlx::query!(
                                r#"
                            SELECT
                                CASE
                                    WHEN EXISTS (
                                        SELECT 1
                                        FROM server_completed_books scb
                                        WHERE scb.volume_id = v.volume_id AND scb.server_id = $2
                                    ) THEN 'Completed'
                                    WHEN EXISTS (
                                        SELECT 1
                                        FROM server_current_book sc
                                        WHERE sc.volume_id = v.volume_id AND sc.server_id = $2
                                    ) THEN 'Currently Reading'
                                    WHEN EXISTS (
                                        SELECT 1
                                        FROM server_book_queue sq
                                        WHERE sq.volume_id = v.volume_id AND sq.server_id = $2
                                    ) THEN 'In Queue'
                                    ELSE 'Not Read'
                                END as status,
                                (
                                    SELECT scb.average_rating::DOUBLE PRECISION
                                    FROM server_completed_books scb
                                    WHERE scb.volume_id = v.volume_id AND scb.server_id = $2
                                    ORDER BY scb.completed_at DESC
                                    LIMIT 1
                                ) as "average_rating?"
                            FROM (SELECT $1::TEXT as volume_id) v
                        "#,
                                volume_id,
                                gid.get() as i64
                            )
                            .fetch_one(pool)
                            .await?;

                            let average_rating = status.average_rating;
                            let mut status_value =
                                status.status.unwrap_or_else(|| "Unknown".to_string());
                            if status_value == "Completed" {
                                if let Some(avg) = average_rating {
                                    status_value = format!("Completed — {:.1}/5 rating", avg);
                                }
                            }

                            let favorites = sqlx::query!(
                            "SELECT COUNT(*) as count FROM get_book_favorites_in_server($1, $2)",
                            volume_id,
                            gid.get() as i64
                        ).fetch_one(pool).await?;

                            (Some(status_value), favorites.count.unwrap_or(0))
                        } else {
                            (None, 0)
                        };

                    let embed = build_book_embed(
                        &book,
                        status_display.as_deref(),
                        favorites_count,
                        Some(QueryMode::Isbn),
                    );
                    ctx.send(poise::CreateReply::default().embed(embed)).await?;
                }
                None => {
                    ctx.say(format!("No book found with ISBN: {}", title_or_isbn))
                        .await?;
                }
            }
        }
        QueryMode::Title => {
            let books = google
                .search_books(&title_or_isbn, author.as_deref(), Some(2))
                .await?;
            if books.is_empty() {
                ctx.say("No books found.").await?;
                return Ok(());
            }
            let book = &books[0];

            if !check_volume_maturity(&ctx, pool, &book).await? {
                // Mature gate failed: tell the user why
                let is_nsfw = current_channel_is_nsfw(&ctx).await?;
                let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;
                let embed = create_mature_content_warning(
                    Some(&book.get_title()),
                    is_nsfw,
                    maturity_enabled,
                );
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            let (status_display, favorites_count) = if let Some(gid) = guild_id {
                let volume_id = &book.id;
                let status = sqlx::query!(
                    r#"
                    SELECT
                        CASE
                            WHEN EXISTS (
                                SELECT 1
                                FROM server_completed_books scb
                                WHERE scb.volume_id = v.volume_id AND scb.server_id = $2
                            ) THEN 'Completed'
                            WHEN EXISTS (
                                SELECT 1
                                FROM server_current_book sc
                                WHERE sc.volume_id = v.volume_id AND sc.server_id = $2
                            ) THEN 'Currently Reading'
                            WHEN EXISTS (
                                SELECT 1
                                FROM server_book_queue sq
                                WHERE sq.volume_id = v.volume_id AND sq.server_id = $2
                            ) THEN 'In Queue'
                            ELSE 'Not Read'
                        END as status,
                        (
                            SELECT scb.average_rating::DOUBLE PRECISION
                            FROM server_completed_books scb
                            WHERE scb.volume_id = v.volume_id AND scb.server_id = $2
                            ORDER BY scb.completed_at DESC
                            LIMIT 1
                        ) as "average_rating?"
                    FROM (SELECT $1::TEXT as volume_id) v
                "#,
                    volume_id,
                    gid.get() as i64
                )
                .fetch_one(pool)
                .await?;

                let average_rating = status.average_rating;
                let mut status_value = status.status.unwrap_or_else(|| "Unknown".to_string());
                if status_value == "Completed" {
                    if let Some(avg) = average_rating {
                        status_value = format!("Completed — {:.1}/5 rating", avg);
                    }
                }

                let favorites = sqlx::query!(
                    "SELECT COUNT(*) as count FROM get_book_favorites_in_server($1, $2)",
                    volume_id,
                    gid.get() as i64
                )
                .fetch_one(pool)
                .await?;

                (Some(status_value), favorites.count.unwrap_or(0))
            } else {
                (None, 0)
            };

            let mut embed = build_book_embed(
                book,
                status_display.as_deref(),
                favorites_count,
                Some(QueryMode::Title),
            );

            if books.len() > 1 {
                // Override footer to include both attributions
                embed = embed.footer(serenity::CreateEmbedFooter::new(
                    "Multiple books found. • Powered by Google Books API",
                ));
            }

            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }

    Ok(())
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Search for books by an author"),
    user_cooldown = 5
)]
pub async fn author(
    ctx: Context<'_>,
    #[description = "The author to search for"] author: String,
) -> Result<(), Error> {
    ctx.defer().await?;

    // Single API call: fetch up to 40 results once; paginate locally
    let google_books = &ctx.data().google_books;
    let books = google_books.search_by_author(&author, Some(40)).await?;

    if books.is_empty() {
        ctx.say("No books found by that author.").await?;
        return Ok(());
    }

    let page_size: usize = 5;
    let total = books.len();
    let total_pages = ((total + page_size - 1) / page_size).max(1);
    let mut page: usize = 0;

    // Build an embed for a given page (0-based)
    let make_embed = |page: usize| {
        let start = page * page_size;
        let end = (start + page_size).min(total);

        let total_display = if total == 40 {
            "40+".to_string()
        } else {
            total.to_string()
        };

        let mut e = CreateEmbed::default()
            .title(format!("Books by {}", author))
            .description(format!("Found {} books", total_display))
            .color(0xB76E79);

        for (i, book) in books[start..end].iter().enumerate() {
            let cats = book.get_categories();
            let categories = if cats.is_empty() {
                "Categories: —".to_string()
            } else {
                format!("Categories: {}", cats.join(", "))
            };
            let info_link = book
                .volume_info
                .info_link
                .clone()
                .unwrap_or_else(|| format!("https://books.google.com/books?id={}", book.id));
            let value = format!("{}\n[Google Books]({})", categories, info_link);
            e = e.field(
                format!("{}. {}", start + i + 1, book.get_title()),
                value,
                false,
            );
        }

        if total > page_size {
            e = e.footer(serenity::CreateEmbedFooter::new(format!(
                "Page {} / {} • showing {}–{} of {} • Powered by Google Books API",
                page + 1,
                total_pages,
                start + 1,
                end,
                total_display
            )));
        } else {
            e = e.footer(serenity::CreateEmbedFooter::new(
                "Powered by Google Books API",
            ));
        }

        e
    };

    // Component row with nav buttons; disable as appropriate
    let make_components = |page: usize| {
        let at_start = page == 0;
        let at_end = page + 1 >= total_pages;

        vec![CreateActionRow::Buttons(vec![
            CreateButton::new("first")
                .label("⮎ First")
                .style(serenity::ButtonStyle::Secondary)
                .disabled(at_start),
            CreateButton::new("prev")
                .label("◀ Prev")
                .style(serenity::ButtonStyle::Secondary)
                .disabled(at_start),
            CreateButton::new("page")
                .label(format!("Page {}/{}", page + 1, total_pages))
                .disabled(true),
            CreateButton::new("next")
                .label("Next ▶")
                .style(serenity::ButtonStyle::Secondary)
                .disabled(at_end),
            CreateButton::new("last")
                .label("Last ⮏")
                .style(serenity::ButtonStyle::Secondary)
                .disabled(at_end),
        ])]
    };

    // Send first page; add buttons only if we have multiple pages
    let reply = poise::CreateReply::default()
        .embed(make_embed(page))
        .components(if total_pages > 1 {
            make_components(page)
        } else {
            vec![]
        });

    let mut msg = ctx.send(reply).await?.into_message().await?;

    if total_pages == 1 {
        return Ok(());
    }

    // Use resettable timeout (resets on each interaction)
    loop {
        let collector = msg
            .await_component_interactions(ctx.serenity_context())
            .author_id(ctx.author().id)
            .timeout(Duration::from_secs(120));

        match collector.next().await {
            Some(mci) => {
                match mci.data.custom_id.as_str() {
                    "first" => page = 0,
                    "prev" => {
                        if page > 0 {
                            page -= 1;
                        }
                    }
                    "next" => {
                        if page + 1 < total_pages {
                            page += 1;
                        }
                    }
                    "last" => page = total_pages.saturating_sub(1),
                    _ => {}
                }

                // Update in-place
                mci.create_response(
                    ctx.serenity_context(),
                    serenity::CreateInteractionResponse::UpdateMessage(
                        serenity::CreateInteractionResponseMessage::default()
                            .embed(make_embed(page))
                            .components(make_components(page)),
                    ),
                )
                .await
                .ok();
            }
            None => {
                // Timed out: disable all buttons
                msg.edit(
                    ctx.serenity_context(),
                    serenity::EditMessage::default()
                        .embed(make_embed(page))
                        .components(
                            make_components(page)
                                .into_iter()
                                .map(|mut row| {
                                    // Disable all buttons in the row
                                    if let CreateActionRow::Buttons(ref mut buttons) = row {
                                        for button in buttons {
                                            *button = button.clone().disabled(true);
                                        }
                                    }
                                    row
                                })
                                .collect(),
                        ),
                )
                .await
                .ok();
                break;
            }
        }
    }

    Ok(())
}
