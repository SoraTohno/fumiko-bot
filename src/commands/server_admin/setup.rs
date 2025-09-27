use crate::{types::Context, types::Error};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter};

#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "DMs you a checklist for configuring the bot (requires Manage Server)",
    ),
    user_cooldown = 10
)]
pub async fn setup(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let dm_channel = match ctx.author().create_dm_channel(&ctx.http()).await {
        Ok(channel) => channel,
        Err(err) => {
            let content = format!(
                "I couldn't send you a DM. Please check your privacy settings. ({})",
                err
            );
            ctx.send(
                poise::CreateReply::default()
                    .content(content)
                    .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
    };

    let embed = CreateEmbed::default()
        .title("Book Club Setup Guide")
        .description("Here's a quick reference for the configuration commands available:")
        .field(
            "Announcements",
            "• `/config announcement set <channel>` — choose where book announcements and polls go.\n• `/config announcement clear` — remove the configured announcement channel.",
            false,
        )
        .field(
            "Queue Access (Default: Enabled)",
            "• `/config queue enable` — allow all members to use `/queue`.\n• `/config queue disable` — limit queue management to admins via `/adminqueue`.",
            false,
        )
        .field(
            "Poll Pinning (Default: Enabled)",
            "• `/config pinning enable` — automatically pin new selection and rating polls.\n• `/config pinning disable` — leave new polls unpinned.",
            false,
        )
        .field(
            "Deadline Automation (Default: Disabled)",
            "• `/config deadline enable` — finish the current book automatically when its deadline passes and open a rating poll.\n• `/config deadline disable` — keep deadlines informational only.",
            false,
        )
        .field(
            "Mature Content Controls",
            "Requires Administrator permission. Mature books can only appear in NSFW (18+) channels when appropriate.\n• `/config mature enable` — allow mature titles in searches, queues, and lists.\n• `/config mature disable` — block mature titles across the bot.\n• `/config mature status` — check whether mature content is currently enabled.",
            false,
        )
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new(
            "Need details? Run /config <section> status to see the current settings.",
        ));

    let _ = dm_channel
        .send_message(
            &ctx.http(),
            poise::serenity_prelude::CreateMessage::new().embed(embed),
        )
        .await;

    ctx.send(
        poise::CreateReply::default()
            .content("I sent you a DM with the setup checklist. If you didn't receive it, please check your privacy settings.")
            .ephemeral(true),
    )
    .await?;

    Ok(())
}
