use crate::maturity_check::{
    check_volume_maturity, current_channel_is_nsfw, server_maturity_enabled,
};
use crate::util::{embed_author_with_icon, get_guild_icon_url, get_guild_name};
use crate::{types::Context, types::Error};
use poise::serenity_prelude::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter,
};
use std::time::Duration;

#[derive(poise::ChoiceParameter, Clone, Copy, Debug)]
pub enum ClubReadSort {
    #[name = "rating"]
    Rating,
    #[name = "date"]
    Date,
}

#[poise::command(
    slash_command,
    guild_only,
    description_localized("en-US", "List all books completed by the server with rankings"),
    user_cooldown = 10
)]
pub async fn clubread(
    ctx: Context<'_>,
    #[description = "Sort by 'rating' or 'date' (default: rating)"] sort: Option<ClubReadSort>,
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
    let sort = sort.unwrap_or(ClubReadSort::Rating);

    match sort {
        ClubReadSort::Rating => {
            // Sorted by rating (UPDATED to use new function with suggested_by)
            let books = sqlx::query!(
                r#"
                SELECT
                rank,
                volume_id,
                suggested_by_username,
                average_rating,
                total_ratings,
                completed_at
                FROM get_server_book_rankings($1)
                "#,
                guild_id.get() as i64
            )
            .fetch_all(pool)
            .await?;

            if books.is_empty() {
                let embed = CreateEmbed::default()
                    .author(embed_author_with_icon(
                        format!("{} Reading History", guild_name),
                        guild_icon.clone(),
                    ))
                    .title("No Books Completed")
                    .description("This server hasn't completed any books yet!")
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            // Fetch book details from Google Books
            let volume_ids: Vec<String> = books
                .iter()
                .map(|b| b.volume_id.clone().unwrap_or_default())
                .collect();
            let volumes_results = google_books.get_volumes_batch(&volume_ids).await;

            // Filter out mature books if they can't be displayed
            let mut filtered_books = Vec::new();
            let mut mature_count = 0;
            let is_nsfw = current_channel_is_nsfw(&ctx).await?;
            let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;

            for (i, book) in books.iter().enumerate() {
                match volumes_results.get(i) {
                    Some(Ok(volume)) => {
                        if check_volume_maturity(&ctx, pool, volume).await? {
                            filtered_books.push((book, Some(volume.clone())));
                        } else {
                            mature_count += 1;
                        }
                    }
                    _ => {
                        // Include books that fail to fetch (API error)
                        filtered_books.push((book, None));
                    }
                }
            }

            if filtered_books.is_empty() && mature_count > 0 {
                let embed = CreateEmbed::default()
                    .author(embed_author_with_icon(
                        format!("{} Reading History", guild_name),
                        guild_icon.clone(),
                    ))
                    .title("Club Reading History")
                    .description(format!(
                        "The server has completed {} book(s), but all are marked as mature content.\n\n\
                        To view mature books, an administrator must enable mature content with `/config mature enable` \
                        and this command must be used in an NSFW channel.",
                        books.len()
                    ))
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Content rating from Google Books API"));
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            let page_size: usize = 5;
            let total = filtered_books.len();
            let total_pages = ((total + page_size - 1) / page_size).max(1);
            let mut page: usize = 0;

            // Build an embed for a given page (0-based)
            let make_embed = |page: usize| {
                let start = page * page_size;
                let end = (start + page_size).min(total);

                let mut embed = CreateEmbed::default()
                    .author(embed_author_with_icon(
                        format!("{} Reading History — Sorted by Rating", guild_name),
                        guild_icon.clone(),
                    ))
                    .description(format!(
                        "Books completed: {}{}",
                        filtered_books.len(),
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

                for i in start..end {
                    let (book, volume_opt) = &filtered_books[i];
                    let rank = book.rank.unwrap_or(0);

                    // Get book details from fetched volumes
                    let (title, authors) = match volume_opt {
                        Some(volume) => (volume.get_title(), volume.get_authors_string()),
                        None => (
                            format!(
                                "Book ({})",
                                book.volume_id.as_ref().unwrap_or(&"Unknown".to_string())
                            ),
                            "Unknown Author".to_string(),
                        ),
                    };

                    let rating = book
                        .average_rating
                        .as_ref()
                        .map(|r| format!("{:.1}/5", r))
                        .unwrap_or_else(|| "No ratings".to_string());
                    let total_ratings = book.total_ratings.unwrap_or(0);
                    let completed_at = book.completed_at.unwrap();
                    let suggested_by = book.suggested_by_username.as_deref().unwrap_or("Unknown");

                    let medal = match rank {
                        1 => "🥇",
                        2 => "🥈",
                        3 => "🥉",
                        _ => "📖",
                    };

                    embed = embed.field(
                        format!("{} #{}. {}", medal, rank, title),
                        format!(
                            "by {}\nSuggested by: {}\nRating: {} ({} votes)\nCompleted: {}",
                            authors,
                            suggested_by,
                            rating,
                            total_ratings,
                            completed_at.format("%B %d, %Y")
                        ),
                        false,
                    );
                }

                if total_pages > 1 {
                    embed = embed.footer(CreateEmbedFooter::new(format!(
                        "Page {} / {} • showing {}–{} of {} • Powered by Google Books API",
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
                                poise::serenity_prelude::CreateInteractionResponseMessage::default(
                                )
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
        }

        ClubReadSort::Date => {
            // Sorted by most recently completed (UPDATED to include suggested_by)
            // updated ao 917 to sort in ASC instead of DSC
            let rows = sqlx::query!(
                r#"
                SELECT 
                    scb.volume_id,
                    du.username as "suggested_by_username?",
                    scb.average_rating,
                    scb.total_ratings,
                    scb.completed_at
                FROM server_completed_books scb
                LEFT JOIN discord_users du ON du.user_id = scb.suggested_by_user_id
                WHERE scb.server_id = $1
                ORDER BY scb.completed_at ASC 
                "#,
                guild_id.get() as i64
            )
            .fetch_all(pool)
            .await?;

            if rows.is_empty() {
                let embed = CreateEmbed::default()
                    .author(embed_author_with_icon(
                        format!("{} Reading History", guild_name),
                        guild_icon.clone(),
                    ))
                    .title("No Books Completed")
                    .description("This server hasn't completed any books yet!")
                    .color(0xB76E79)
                    .footer(CreateEmbedFooter::new("Powered by Google Books API"));
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            // Fetch book details from Google Books
            let volume_ids: Vec<String> = rows.iter().map(|b| b.volume_id.clone()).collect();
            let volumes_results = google_books.get_volumes_batch(&volume_ids).await;

            // Filter out mature books if they can't be displayed
            let mut filtered_rows = Vec::new();
            let mut mature_count = 0;
            let is_nsfw = current_channel_is_nsfw(&ctx).await?;
            let maturity_enabled = server_maturity_enabled(&ctx, pool).await?;

            for (i, row) in rows.iter().enumerate() {
                match volumes_results.get(i) {
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
                        format!("{} Reading History", guild_name),
                        guild_icon.clone(),
                    ))
                    .title("Club Reading History")
                    .description(format!(
                        "The server has completed {} book(s), but all are marked as mature content.\n\n\
                        To view mature books, an administrator must enable mature content with `/config mature enable` \
                        and this command must be used in an NSFW channel.",
                        rows.len()
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

                let mut embed = CreateEmbed::default()
                    .author(embed_author_with_icon(
                        format!("{} Reading History — Sorted by Date", guild_name),
                        guild_icon.clone(),
                    ))
                    .description(format!(
                        "Books completed: {}{}",
                        filtered_rows.len(),
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

                for i in start..end {
                    let (book, volume_opt) = &filtered_rows[i];
                    let n = i + 1; // Global numbering

                    // Get book details from fetched volumes
                    let (title, authors) = match volume_opt {
                        Some(volume) => (volume.get_title(), volume.get_authors_string()),
                        None => (
                            format!("Book ({})", book.volume_id),
                            "Unknown Author".to_string(),
                        ),
                    };

                    let rating = book
                        .average_rating
                        .as_ref()
                        .map(|r| format!("{:.1}/5", r))
                        .unwrap_or_else(|| "No ratings".to_string());
                    let total_ratings = book.total_ratings.unwrap_or(0);
                    let completed_at = book.completed_at.unwrap();
                    let suggested_by = book.suggested_by_username.as_deref().unwrap_or("Unknown");

                    embed = embed.field(
                        format!("{}. {}", n, title),
                        format!(
                            "by {}\nSuggested by: {}\nRating: {} ({} votes)\nCompleted: {}",
                            authors,
                            suggested_by,
                            rating,
                            total_ratings,
                            completed_at.format("%B %d, %Y")
                        ),
                        false,
                    );
                }

                if total_pages > 1 {
                    embed = embed.footer(CreateEmbedFooter::new(format!(
                        "Page {} / {} • showing {}–{} of {} • Powered by Google Books API",
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
                                poise::serenity_prelude::CreateInteractionResponseMessage::default(
                                )
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
        }
    }

    Ok(())
}
