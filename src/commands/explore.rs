use crate::types::{Context, Data, Error};
use poise::serenity_prelude as serenity;
use poise::serenity_prelude::{CreateActionRow, CreateButton, CreateEmbed};
use std::time::Duration;

type CommandVec = Vec<poise::Command<Data, Error>>;

pub fn explore_commands() -> CommandVec {
    vec![explore()]
}

fn build_search_summary(
    query: Option<&str>,
    author: Option<&str>,
    genre: Option<&str>,
    publisher: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    if let Some(q) = query {
        parts.push(format!("\"{}\"", q));
    }
    if let Some(a) = author {
        parts.push(format!("Author: {}", a));
    }
    if let Some(g) = genre {
        parts.push(format!("Genre: {}", g));
    }
    if let Some(p) = publisher {
        parts.push(format!("Publisher: {}", p));
    }

    parts.join(" • ")
}

#[poise::command(
    slash_command,
    description_localized("en-US", "Explore books with the Google Books API"),
    user_cooldown = 5
)]
pub async fn explore(
    ctx: Context<'_>,
    #[description = "General search query"] query: Option<String>,
    #[description = "Filter by author"] author: Option<String>,
    #[description = "Filter by genre or subject"] genre: Option<String>,
    #[description = "Filter by publisher"] publisher: Option<String>,
) -> Result<(), Error> {
    let query = query
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let author = author
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let genre = genre
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let publisher = publisher
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if query.is_none() && author.is_none() && genre.is_none() && publisher.is_none() {
        ctx.say("Please provide at least one search parameter.")
            .await?;
        return Ok(());
    }

    ctx.defer().await?;

    let google_books = &ctx.data().google_books;
    let books = google_books
        .search(
            query.as_deref(),
            author.as_deref(),
            genre.as_deref(),
            publisher.as_deref(),
            Some(40),
        )
        .await?;

    if books.is_empty() {
        ctx.say("No books found for that search.").await?;
        return Ok(());
    }

    let page_size: usize = 5;
    let total = books.len();
    let total_pages = ((total + page_size - 1) / page_size).max(1);
    let mut page: usize = 0;

    let search_summary = build_search_summary(
        query.as_deref(),
        author.as_deref(),
        genre.as_deref(),
        publisher.as_deref(),
    );
    let embed_title = if search_summary.is_empty() {
        "Explore Books".to_string()
    } else {
        format!("Results for {}", search_summary)
    };

    let make_embed = |page: usize| {
        let start = page * page_size;
        let end = (start + page_size).min(total);

        let total_display = if total == 40 {
            "40+".to_string()
        } else {
            total.to_string()
        };

        let mut e = CreateEmbed::default()
            .title(embed_title.clone())
            .description(format!("Found {} books", total_display))
            .color(0xB76E79);

        for (i, book) in books[start..end].iter().enumerate() {
            let info_link = book
                .volume_info
                .info_link
                .clone()
                .unwrap_or_else(|| format!("https://books.google.com/books?id={}", book.id));
            let mut lines = vec![format!("by {}", book.get_authors_string())];
            if let Some(publisher) = &book.volume_info.publisher {
                lines.push(format!("Publisher: {}", publisher));
            }
            if let Some(published_date) = &book.volume_info.published_date {
                lines.push(format!("Published: {}", published_date));
            }
            let categories = book.get_categories();
            if !categories.is_empty() {
                lines.push(format!("Categories: {}", categories.join(", ")));
            }
            lines.push(format!("[Google Books]({})", info_link));

            e = e.field(
                format!("{}. {}", start + i + 1, book.get_title()),
                lines.join("\n"),
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
                msg.edit(
                    ctx.serenity_context(),
                    serenity::EditMessage::default()
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
