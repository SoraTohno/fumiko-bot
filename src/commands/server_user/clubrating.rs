use crate::maturity_check::{
    check_volume_maturity, create_mature_content_warning, current_channel_is_nsfw,
    server_maturity_enabled,
};
use crate::types::QueryMode;
use crate::util::{
    detect_query_mode, embed_author_with_icon, get_guild_icon_url, get_guild_name, normalize_isbn,
};
use crate::{types::Context, types::Error};
use poise::serenity_prelude::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter,
};
use std::time::Duration;

#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "Show ratings for a specific book"),
    user_cooldown = 10
)]
pub async fn clubrating(
    ctx: Context<'_>,
    #[description = "Title or ISBN-10/13 to check ratings for"] title_or_isbn: String,
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
    let guild_icon = get_guild_icon_url(&ctx).await;

    // Search for the book using Google Books API
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
    let book_label = format!("'{}'", book_title);

    let author_label = format!("Ratings for {}", book_label);
    let author_name = book.get_authors_string();

    let author_display = {
        let a = author_name.trim();
        if a.is_empty() {
            "Unknown author".to_string()
        } else {
            author_name.clone()
        }
    };

    let thumbnail_url = book.get_thumbnail_url();

    if !check_volume_maturity(&ctx, pool, &book).await? {
        let is_nsfw = current_channel_is_nsfw(&ctx).await?;
        let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;
        let embed = create_mature_content_warning(Some(&book_title), is_nsfw, maturity_enabled);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    // Check if book was completed by this server
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

    match completed_book {
        Some(book_info) => {
            // Show all ratings for the most recent completion of this book
            let ratings = sqlx::query!(
                r#"
                SELECT 
                    du.username,
                    ubr.rating,
                    ubr.rated_at
                FROM user_book_ratings ubr
                JOIN discord_users du ON du.user_id = ubr.user_id
                WHERE ubr.completed_id = $1
                ORDER BY ubr.rating DESC, ubr.rated_at ASC
                "#,
                book_info.completed_id
            )
            .fetch_all(pool)
            .await?;

            // let mut embed = CreateEmbed::default()
            //     .title(format!("Ratings for '{}'", book_title))
            //     .color(0xB76E79);

            // if let Some(avg_rating) = book_info.average_rating.as_ref() {
            //     embed = embed.field(
            //         "Average Rating",
            //         format!("{:.1}/5 ({} ratings)", avg_rating, book_info.total_ratings.unwrap_or(0)),
            //         false
            //     );
            // }

            // Pagination: show 10 per page with buttons if there are more than 10 ratings
            let total = ratings.len();
            let page_size: usize = 10;
            let total_pages = ((total + page_size - 1) / page_size).max(1);
            let mut page: usize = 0;

            // Precompute average rating display to avoid moving values into the closure
            let avg_display: Option<String> = book_info.average_rating.map(|avg| {
                format!(
                    "{:.1}/5 ({} ratings)",
                    avg,
                    book_info.total_ratings.unwrap_or(0)
                )
            });

            let make_embed = |page: usize| {
                let mut e = CreateEmbed::default()
                    .author(embed_author_with_icon(
                        author_label.clone(),
                        guild_icon.clone(),
                    ))
                    .color(0xB76E79)
                    .field("Authors", author_display.clone(), false);

                if let Some(avg) = avg_display.as_ref() {
                    e = e.field("Average Rating", avg.clone(), false);
                }

                if ratings.is_empty() {
                    e = e.description("No ratings yet!");
                    let footer_text = if result_bool {
                        "Multiple books found. • Powered by Google Books API"
                    } else {
                        "Powered by Google Books API"
                    };
                    if let Some(thumbnail) = thumbnail_url.as_ref() {
                        e = e.thumbnail(thumbnail.clone());
                    }
                    return e.footer(CreateEmbedFooter::new(footer_text));
                }

                let start = page * page_size;
                let end = (start + page_size).min(total);
                let mut ratings_text = String::new();
                for rating in &ratings[start..end] {
                    ratings_text
                        .push_str(&format!("**{}**: {}/5\n", rating.username, rating.rating));
                }
                e = e.field("Individual Ratings", ratings_text, false);

                let footer_text = if total_pages > 1 {
                    format!(
                        "Page {}/{} • Powered by Google Books API",
                        page + 1,
                        total_pages
                    )
                } else if result_bool {
                    format!("Multiple books found. • Powered by Google Books API")
                } else {
                    format!("Powered by Google Books API")
                };
                if let Some(thumbnail) = thumbnail_url.as_ref() {
                    e = e.thumbnail(thumbnail.clone());
                }
                e.footer(CreateEmbedFooter::new(footer_text))
            };

            let make_components = |page: usize| {
                if total_pages <= 1 {
                    return vec![];
                }
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
                        .disabled(true)
                        .style(ButtonStyle::Secondary),
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
                .components(make_components(page));
            let mut msg = ctx.send(reply).await?.into_message().await?;

            if total_pages > 1 {
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
                                        .components(make_components(page))
                                )
                            ).await.ok();
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
                                                if let CreateActionRow::Buttons(ref mut buttons) =
                                                    row
                                                {
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
            }
        }
        None => {
            let embed = CreateEmbed::default()
                .author(embed_author_with_icon(
                    author_label.clone(),
                    guild_icon.clone(),
                ))
                .title("Not Yet Read")
                .description(format!(
                    "'{}' hasn't been read by this book club yet.",
                    book_title
                ))
                .color(0xB76E79)
                .field("Authors", author_display, false)
                .footer(CreateEmbedFooter::new("Powered by Google Books API"));
            let embed = if let Some(thumbnail) = thumbnail_url.as_ref() {
                embed.thumbnail(thumbnail.clone())
            } else {
                embed
            };
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }

    Ok(())
}
