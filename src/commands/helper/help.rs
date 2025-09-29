use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, CreateMessage};

use crate::{types::Context, types::Error, util::embed_author_with_icon};

type BotCommand = poise::Command<crate::types::Data, crate::types::Error>;

fn command_description(command: &BotCommand) -> String {
    command
        .description
        .as_deref()
        .or_else(|| {
            command
                .description_localizations
                .get("en-US")
                .map(|s| s.as_str())
        })
        .unwrap_or("No description provided.")
        .to_string()
}

fn collect_entries(command: &BotCommand, prefix: &str, output: &mut Vec<String>) {
    if command.hide_in_help {
        return;
    }

    let name = if prefix.is_empty() {
        format!("/{}", command.name)
    } else {
        format!("{} {}", prefix, command.name)
    };

    let mut description = command_description(command);

    if command.subcommands.is_empty() {
        output.push(format!("• **{}** — {}", name, description));
        return;
    }

    if description.trim().is_empty() {
        description = "Choose one of the subcommands.".to_string();
    } else {
        description.push_str(" (choose a subcommand)");
    }

    output.push(format!("• **{}** — {}", name, description));

    for sub in &command.subcommands {
        collect_entries(sub, &name, output);
    }
}

#[poise::command(
    slash_command,
    description_localized("en-US", "DM you a list of every command the bot offers"),
    user_cooldown = 5
)]
pub async fn help(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let mut sections: Vec<(String, String)> = Vec::new();

    for command in &ctx.framework().options().commands {
        if command.hide_in_help {
            continue;
        }

        let mut lines = Vec::new();
        collect_entries(command, "", &mut lines);

        if lines.is_empty() {
            continue;
        }

        let mut chunks = Vec::new();
        let mut current = String::new();
        for line in lines {
            if current.len() + line.len() + 1 > 1024 {
                if !current.is_empty() {
                    chunks.push(current);
                }
                current = String::new();
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(&line);
        }
        if !current.is_empty() {
            chunks.push(current);
        }

        for (index, chunk) in chunks.into_iter().enumerate() {
            let mut title = format!("/{}", command.name);
            if index > 0 {
                title = format!("{} (continued {})", title, index + 1);
            }
            sections.push((title, chunk));
        }
    }

    if sections.is_empty() {
        let embed = CreateEmbed::default()
            .title("No commands available")
            .description("I couldn't find any commands to list right now.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
            .await?;
        return Ok(());
    }

    let bot_face = ctx.serenity_context().http.get_current_user().await?.face();

    let dm_channel = match ctx.author().create_dm_channel(&ctx.http()).await {
        Ok(channel) => channel,
        Err(_) => {
            let embed = CreateEmbed::default()
                .title("❌ Couldn't send DM")
                .description(
                    "I couldn't send you a DM. Please make sure your DMs are open and try again.",
                )
                .color(0xB76E79);
            ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
                .await?;
            return Ok(());
        }
    };

    const MAX_FIELDS_PER_PAGE: usize = 25;
    const MAX_EMBED_CHARS: usize = 5_000;

    let mut pages: Vec<Vec<(String, String)>> = Vec::new();
    let mut current_page: Vec<(String, String)> = Vec::new();
    let mut current_length = 0usize;

    for (name, value) in sections.into_iter() {
        let entry_length = name.len() + value.len();

        if !current_page.is_empty()
            && (current_page.len() >= MAX_FIELDS_PER_PAGE
                || current_length + entry_length > MAX_EMBED_CHARS)
        {
            pages.push(current_page);
            current_page = Vec::new();
            current_length = 0;
        }

        current_length += entry_length;
        current_page.push((name, value));
    }

    if !current_page.is_empty() {
        pages.push(current_page);
    }

    let page_count = pages.len();

    for (page_index, chunk) in pages.into_iter().enumerate() {
        let mut dm_embed = CreateEmbed::default()
            .author(embed_author_with_icon(
                "Fumiko Book Club Bot Commands",
                Some(bot_face.clone()),
            ))
            .description(
                "Here's everything I can do. Commands with subcommands list every available option.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new(if page_count > 1 {
                format!(
                    "Use /help anytime to see this list again. • https://fumiko.dev • Page {} of {}",
                    page_index + 1,
                    page_count
                )
            } else {
                "Use /help anytime to see this list again. • https://fumiko.dev".to_string()
            }));

        for (name, value) in chunk {
            dm_embed = dm_embed.field(name, value, false);
        }

        if let Err(err) = dm_channel
            .send_message(&ctx.http(), CreateMessage::new().embed(dm_embed))
            .await
        {
            let embed = CreateEmbed::default()
                .title("❌ Couldn't send DM")
                .description(format!(
                    "I couldn't send you a DM. Please make sure your DMs are open and try again. ({err})",
                ))
                .color(0xB76E79);
            ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
                .await?;
            return Ok(());
        }
    }

    let info_embed = CreateEmbed::default()
        .description(
            "• Use `/help` or visit [fumiko.dev/commands](https://fumiko.dev/commands) to view this list of commands again.\n\
            • Visit [fumiko.dev/guide](https://fumiko.dev/guide) for an explanation on how to use Fumiko bot.\n\
            • Please visit [fumiko.dev/contact](https://fumiko.dev/contact) for additionalif you require assistance or have any questions.",
        )
        .color(0xB76E79);

    if let Err(err) = dm_channel
        .send_message(&ctx.http(), CreateMessage::new().embed(info_embed))
        .await
    {
        let embed = CreateEmbed::default()
            .title("❌ Couldn't send DM")
            .description(format!(
                "I couldn't send you a DM. Please make sure your DMs are open and try again. ({err})",
            ))
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
            .await?;
        return Ok(());
    }

    let embed = CreateEmbed::default()
        .title("✅ Help sent")
        .description("Check your DMs for the full list of commands.")
        .color(0xB76E79);
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
        .await?;

    Ok(())
}
