use crate::maturity_check::{
    check_volume_maturity, create_mature_content_warning, current_channel_is_nsfw,
    server_maturity_enabled,
};
use crate::types::QueryMode;
use crate::util::{detect_query_mode, embed_author_with_icon, normalize_isbn};
use crate::{types::Context, types::Error};
use poise::serenity_prelude::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter, User,
};
use sqlx::types::chrono::{DateTime, Utc};
use std::time::Duration;

#[derive(poise::ChoiceParameter, Clone, Copy, Debug)]
pub enum UserRatingSort {
    #[name = "rating"]
    Rating,
    #[name = "date"]
    Date,
}

#[derive(Debug)]
struct UserRatingRow {
    volume_id: Option<String>,
    rating: Option<i32>,
    rated_at: Option<DateTime<Utc>>,
}

#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "See the ratings a user has given in this server"),
    user_cooldown = 10
)]
pub async fn userrating(
    ctx: Context<'_>,
    #[description = "User to show (defaults to you)"] user: Option<User>,
    #[description = "Optionally, filter to a specific title or ISBN"] title_or_isbn: Option<String>,
    #[description = "Optional author to disambiguate the title (used when title)"] author: Option<
        String,
    >,
    #[description = "Sort by 'rating' or 'date' (default: rating)"] sort: Option<UserRatingSort>,
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
    let target_user = user.unwrap_or_else(|| ctx.author().clone());
    let sort = sort.unwrap_or(UserRatingSort::Rating);

    // If a query is provided, show this user's rating for that specific book
    if let Some(title_or_isbn) = title_or_isbn {
        // Search for the book using unified logic
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
                    // Will note in footer
                    result_bool = true;
                }

                let selected = results.into_iter().next().unwrap();

                selected
            }
        };

        let volume_id = &book.id;
        let book_title = book.get_title();
        let thumbnail_url = book.get_thumbnail_url();

        if !check_volume_maturity(&ctx, pool, &book).await? {
            let is_nsfw = current_channel_is_nsfw(&ctx).await?;
            let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;
            let embed = create_mature_content_warning(Some(&book_title), is_nsfw, maturity_enabled);
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
            return Ok(());
        }

        let completed = sqlx::query!(
            r#"
            SELECT completed_id
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

        if let Some(c) = completed {
            let user_rating = sqlx::query!(
                "SELECT rating, rated_at
                 FROM user_book_ratings
                 WHERE user_id = $1 AND completed_id = $2",
                target_user.id.get() as i64,
                c.completed_id
            )
            .fetch_optional(pool)
            .await?;

            match user_rating {
                Some(r) => {
                    let mut embed = CreateEmbed::default()
                        .author(embed_author_with_icon(
                            format!("{}'s Rating", target_user.name),
                            Some(target_user.face()),
                        ))
                        .field("Book", &book_title, false)
                        .field("Rating", format!("{}/5", r.rating), true)
                        .color(0xB76E79);
                    if let Some(ts) = r.rated_at {
                        embed = embed.field("Rated on", ts.format("%B %d, %Y").to_string(), true);
                    }

                    let footer_text = if result_bool {
                        "Multiple books found. • Powered by Google Books API"
                    } else {
                        "Powered by Google Books API"
                    };
                    embed = embed.footer(CreateEmbedFooter::new(footer_text));

                    if let Some(url) = thumbnail_url.as_deref() {
                        embed = embed.image(url);
                    }

                    ctx.send(poise::CreateReply::default().embed(embed)).await?;
                }
                None => {
                    let mut embed = CreateEmbed::default()
                        .author(embed_author_with_icon(
                            format!("{}'s Rating", target_user.name),
                            Some(target_user.face()),
                        ))
                        .title("No Rating")
                        .description(format!(
                            "{} hasn't rated *{}* yet.",
                            target_user.name, &book_title
                        ))
                        .color(0xB76E79)
                        .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                    if let Some(url) = thumbnail_url.as_deref() {
                        embed = embed.image(url);
                    }
                    ctx.send(poise::CreateReply::default().embed(embed)).await?;
                }
            }
        } else {
            let mut embed = CreateEmbed::default()
                .title("Not Yet Read")
                .description(format!(
                    "*{}* hasn't been read by this book club yet.",
                    &book_title
                ))
                .color(0xB76E79)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));
            if let Some(url) = thumbnail_url.as_deref() {
                embed = embed.image(url);
            }
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }

        return Ok(());
    }

    // Otherwise, list everything this user has rated in this server
    let rows: Vec<UserRatingRow> = match sort {
        UserRatingSort::Date => {
            sqlx::query_as!(
                UserRatingRow,
                r#"
                SELECT volume_id, rating, rated_at
                FROM server_book_ratings_view
                WHERE server_id = $1 AND user_id = $2
                ORDER BY rated_at DESC NULLS LAST
                "#,
                guild_id.get() as i64,
                target_user.id.get() as i64
            )
            .fetch_all(pool)
            .await?
        }
        UserRatingSort::Rating => {
            sqlx::query_as!(
                UserRatingRow,
                r#"
                SELECT volume_id, rating, rated_at
                FROM server_book_ratings_view
                WHERE server_id = $1 AND user_id = $2
                ORDER BY rating DESC NULLS LAST, rated_at ASC
                "#,
                guild_id.get() as i64,
                target_user.id.get() as i64
            )
            .fetch_all(pool)
            .await?
        }
    };

    if rows.is_empty() {
        let embed = CreateEmbed::default()
            .author(embed_author_with_icon(
                format!("{}'s Ratings", target_user.name),
                Some(target_user.face()),
            ))
            .title("No Ratings Yet")
            .description(format!(
                "{} hasn't rated any completed books in this server yet.",
                target_user.name
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Fetch book details from Google Books
    let volume_ids: Vec<String> = rows.iter().filter_map(|r| r.volume_id.clone()).collect();
    let volumes = google_books.get_volumes_batch(&volume_ids).await;

    // Filter out mature books if they can't be displayed
    let mut filtered_rows = Vec::new();
    let mut mature_count = 0;
    let is_nsfw = current_channel_is_nsfw(&ctx).await?;
    let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;

    for (i, row) in rows.iter().enumerate() {
        match volumes.get(i) {
            Some(Ok(volume)) => {
                if check_volume_maturity(&ctx, pool, volume).await? {
                    filtered_rows.push((row, Some(volume.clone())));
                } else {
                    mature_count += 1;
                }
            }
            _ => {
                // Include books that fail to fetch (API error)
                filtered_rows.push((row, None));
            }
        }
    }

    if filtered_rows.is_empty() && mature_count > 0 {
        let embed = CreateEmbed::default()
            .author(embed_author_with_icon(
                format!("{}'s Ratings", target_user.name),
                Some(target_user.face()),
            ))
            .title("No Visible Ratings")
            .description(format!(
                "{} has rated {} book(s) in this server, but all are marked as mature content.\n\n\
                 To view mature books, an administrator must enable mature content with `/config mature enable` \
                 and this command must be used in an NSFW channel.",
                target_user.name, rows.len()
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Content rating from Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let page_size: usize = 5;
    let total = filtered_rows.len();
    let total_pages = ((total + page_size - 1) / page_size).max(1);
    let mut page: usize = 0;

    // Build an embed for a given page (0-based)
    let make_embed = |page: usize| {
        let start = page * page_size;
        let end = (start + page_size).min(total);

        let mut list = String::new();

        for i in start..end {
            let (r, volume_opt) = &filtered_rows[i];
            let n = i + 1; // Global numbering
            let when = r
                .rated_at
                .map(|t| t.format("%b %d, %Y").to_string())
                .unwrap_or_else(|| "date unknown".to_string());

            let title = match volume_opt {
                Some(volume) => volume.get_title(),
                None => format!(
                    "Book ({})",
                    r.volume_id.as_ref().unwrap_or(&"Unknown".to_string())
                ),
            };

            list.push_str(&format!(
                "{n}. **{}** — {}/5 _(rated {})_\n",
                title,
                r.rating.unwrap_or(0),
                when
            ));
        }

        let mut embed = CreateEmbed::default()
            .author(embed_author_with_icon(
                format!("{}'s Ratings", target_user.name),
                Some(target_user.face()),
            ))
            .description(format!(
                "{}{}",
                list,
                if mature_count > 0 && page == 0 {
                    let mut why: Vec<&str> = Vec::new();
                    if !is_nsfw {
                        why.push("blocked in non-NSFW channel");
                    }
                    if !maturity_enabled {
                        why.push("maturity disabled");
                    }
                    if why.is_empty() {
                        format!("\n_({} mature books hidden)_", mature_count)
                    } else {
                        format!(
                            "\n_({} mature books hidden — {})_",
                            mature_count,
                            why.join(" & ")
                        )
                    }
                } else {
                    String::new()
                }
            ))
            .color(0xB76E79);

        if total_pages > 1 {
            let label = match sort {
                UserRatingSort::Date => "Showing latest",
                UserRatingSort::Rating => "Showing top",
            };
            embed = embed.footer(CreateEmbedFooter::new(format!(
                "{} — Page {} / {} • showing {}–{} of {} • Powered by Google Books API",
                label,
                page + 1,
                total_pages,
                start + 1,
                end,
                total
            )));
        } else {
            embed = embed.footer(CreateEmbedFooter::new("Powered by Google Books API"));
        }

        embed
    };

    // Component row with nav buttons
    let make_components = |page: usize| {
        let at_start = page == 0;
        let at_end = page + 1 >= total_pages;

        vec![CreateActionRow::Buttons(vec![
            CreateButton::new("first")
                .label("⮎ First")
                .style(ButtonStyle::Secondary)
                .disabled(at_start),
            CreateButton::new("prev")
                .label("◀ Prev")
                .style(ButtonStyle::Secondary)
                .disabled(at_start),
            CreateButton::new("page")
                .label(format!("Page {}/{}", page + 1, total_pages))
                .disabled(true),
            CreateButton::new("next")
                .label("Next ▶")
                .style(ButtonStyle::Secondary)
                .disabled(at_end),
            CreateButton::new("last")
                .label("Last ⮏")
                .style(ButtonStyle::Secondary)
                .disabled(at_end),
        ])]
    };

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

                mci.create_response(
                    ctx.serenity_context(),
                    poise::serenity_prelude::CreateInteractionResponse::UpdateMessage(
                        poise::serenity_prelude::CreateInteractionResponseMessage::default()
                            .embed(make_embed(page))
                            .components(make_components(page)),
                    ),
                )
                .await
                .ok();
            }
            None => {
                // Timeout reached - disable buttons
                msg.edit(
                    ctx.serenity_context(),
                    poise::serenity_prelude::EditMessage::default()
                        .embed(make_embed(page))
                        .components(
                            make_components(page)
                                .into_iter()
                                .map(|mut row| {
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
