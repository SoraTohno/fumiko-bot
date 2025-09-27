use crate::ensure_server_exists;
use crate::util::{
    auto_complete_on_deadline_enabled, get_guild_name, pin_polls_enabled, queue_commands_enabled,
};
use crate::{types::Context, types::Error};
use poise::serenity_prelude::{
    Channel, ChannelId, ChannelType, CreateEmbed, CreateEmbedFooter, Mentionable,
};

#[poise::command(
    slash_command,
    subcommands("announcement", "queue", "pinning", "deadline", "mature"),
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Configure bot settings for this server (requires Manage Server)",
    ),
    user_cooldown = 10
)]
pub async fn config(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    subcommands("set", "clear", "status"),
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Manage the announcement channel used for book club updates (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn announcement(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Set the channel used for book announcements (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn set(
    ctx: Context<'_>,
    #[description = "Channel where the bot should post announcements"] channel: Channel,
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

    let guild_channel = match channel {
        Channel::Guild(channel) => channel,
        _ => {
            let embed = CreateEmbed::default()
                .title("❌ Unsupported Channel")
                .description("Please pick a text or announcement channel from this server.")
                .color(0xB76E79);
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    };

    if guild_channel.guild_id != guild_id {
        let embed = CreateEmbed::default()
            .title("❌ Channel Not in Server")
            .description("Please choose a channel that belongs to this server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let channel_id: ChannelId = match guild_channel.kind {
        ChannelType::Text | ChannelType::News => guild_channel.id,
        _ => {
            let embed = CreateEmbed::default()
                .title("❌ Unsupported Channel Type")
                .description("Announcements can only be posted in text or announcement channels.")
                .color(0xB76E79);
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    };

    let pool = &ctx.data().database;
    let guild_name = get_guild_name(&ctx).await;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    sqlx::query!(
        "INSERT INTO server_bot_config (server_id, announcement_channel_id)
         VALUES ($1, $2)
         ON CONFLICT (server_id)
         DO UPDATE SET announcement_channel_id = $2, updated_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64,
        channel_id.get() as i64
    )
    .execute(pool)
    .await?;

    let embed = CreateEmbed::default()
        .title("✅ Announcement Channel Set")
        .description(format!(
            "I'll share book announcements in {} from now on.",
            channel_id.mention()
        ))
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;

    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Clear the configured announcement channel (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn clear(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;

    let existing = sqlx::query!(
        "SELECT announcement_channel_id FROM server_bot_config WHERE server_id = $1",
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    if existing
        .as_ref()
        .and_then(|row| row.announcement_channel_id)
        .is_none()
    {
        let embed = CreateEmbed::default()
            .title("No Announcement Channel Set")
            .description("Use `/config announcement set` to choose a channel for announcements.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    sqlx::query!(
        "UPDATE server_bot_config
         SET announcement_channel_id = NULL, updated_at = CURRENT_TIMESTAMP
         WHERE server_id = $1",
        guild_id.get() as i64
    )
    .execute(pool)
    .await?;

    let embed = CreateEmbed::default()
        .title("Announcement Channel Cleared")
        .description(
            "Announcements will no longer be posted automatically until a new channel is set.",
        )
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;

    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "View the current announcement channel (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn status(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;

    let current = sqlx::query!(
        "SELECT announcement_channel_id FROM server_bot_config WHERE server_id = $1",
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    let embed = if let Some(channel_id) = current.and_then(|row| row.announcement_channel_id) {
        let channel_id = ChannelId::new(channel_id as u64);
        CreateEmbed::default()
            .title("Announcement Channel Configured")
            .description(format!(
                "Announcements are currently sent to {}. Use `/config announcement clear` to remove it.",
                channel_id.mention()
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"))
    } else {
        CreateEmbed::default()
            .title("Announcement Channel Not Set")
            .description("No announcement channel is configured. Use `/config announcement set` to choose one.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"))
    };

    ctx.send(poise::CreateReply::default().embed(embed)).await?;

    Ok(())
}

#[poise::command(
    slash_command,
    subcommands("deadline_enable", "deadline_disable", "deadline_status"),
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Control whether books finish automatically when their deadline passes",
    ),
    user_cooldown = 10
)]
async fn deadline(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "enable",
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Automatically finish the current book when its deadline arrives",
    ),
    user_cooldown = 10
)]
async fn deadline_enable(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;
    let guild_name = get_guild_name(&ctx).await;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    if auto_complete_on_deadline_enabled(pool, guild_id.get() as i64).await? {
        let embed = CreateEmbed::default()
            .title("Deadline Auto-Completion Already Enabled")
            .description(
                "I'll finish the current book automatically when its deadline passes. Use `/config deadline disable` to turn this off.",
            )
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    sqlx::query!(
        "INSERT INTO server_bot_config (server_id, auto_complete_on_deadline)
         VALUES ($1, $2)
         ON CONFLICT (server_id)
         DO UPDATE SET auto_complete_on_deadline = $2, updated_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64,
        true
    )
    .execute(pool)
    .await?;

    let embed = CreateEmbed::default()
        .title("Deadline Auto-Completion Enabled")
        .description(
            "When a selected book reaches its deadline, I'll mark it as finished and start the rating poll automatically.",
        )
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new(
            "Set deadlines with /select commands like /select manual",
        ));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "disable",
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Stop books from finishing automatically when their deadline passes",
    ),
    user_cooldown = 10
)]
async fn deadline_disable(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;

    if !auto_complete_on_deadline_enabled(pool, guild_id.get() as i64).await? {
        let embed = CreateEmbed::default()
            .title("Deadline Auto-Completion Already Disabled")
            .description(
                "Deadlines won't finish books automatically right now. Use `/config deadline enable` if you change your mind.",
            )
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    sqlx::query!(
        "INSERT INTO server_bot_config (server_id, auto_complete_on_deadline)
         VALUES ($1, $2)
         ON CONFLICT (server_id)
         DO UPDATE SET auto_complete_on_deadline = $2, updated_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64,
        false
    )
    .execute(pool)
    .await?;

    let embed = CreateEmbed::default()
        .title("Deadline Auto-Completion Disabled")
        .description(
            "I'll leave the current book alone when its deadline passes. Run `/finishbook` whenever you're ready to wrap it up.",
        )
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Use /config deadline enable to restore automatic finishing"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "status",
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Check whether books finish automatically at deadlines (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn deadline_status(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;
    let enabled = auto_complete_on_deadline_enabled(pool, guild_id.get() as i64).await?;

    let embed = if enabled {
        CreateEmbed::default()
            .title("✅ Deadline Auto-Completion Enabled")
            .description(
                "Books with deadlines will finish automatically when the date arrives and a rating poll will be created.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new(
                "Use `/config deadline disable` to manage this setting",
            ))
    } else {
        CreateEmbed::default()
            .title("Deadline Auto-Completion Disabled")
            .description(
                "Deadlines are informational only right now. Run `/config deadline enable` if you'd like me to finish books automatically.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Configure with /config deadline"))
    };

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    subcommands("queue_disable", "queue_enable", "queue_status"),
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Enable/disable /queue add/remove; restrict queue management to /adminqueue. (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn queue(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "disable",
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Disable the /queue command for everyone (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn queue_disable(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;
    let guild_name = get_guild_name(&ctx).await;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    if !queue_commands_enabled(pool, guild_id.get() as i64).await? {
        let embed = CreateEmbed::default()
            .title("Queue Command Already Disabled")
            .description("Members cannot use `/queue` right now. Use `/config queue enable` to turn it back on.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    sqlx::query!(
        "INSERT INTO server_bot_config (server_id, queue_enabled)
         VALUES ($1, $2)
         ON CONFLICT (server_id)
         DO UPDATE SET queue_enabled = $2, updated_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64,
        false
    )
    .execute(pool)
    .await?;

    let embed = CreateEmbed::default()
        .title("Queue Command Disabled")
        .description(
            "The public `/queue` command is now disabled. Members with the Manage Messages permission can still use `/adminqueue`.",
        )
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "enable",
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Enable the /queue command for everyone (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn queue_enable(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;
    let guild_name = get_guild_name(&ctx).await;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    if queue_commands_enabled(pool, guild_id.get() as i64).await? {
        let embed = CreateEmbed::default()
            .title("Queue Command Already Enabled")
            .description(
                "Members can already use `/queue`. Use `/config queue disable` to turn it off.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    sqlx::query!(
        "INSERT INTO server_bot_config (server_id, queue_enabled)
         VALUES ($1, $2)
         ON CONFLICT (server_id)
         DO UPDATE SET queue_enabled = $2, updated_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64,
        true
    )
    .execute(pool)
    .await?;

    let embed = CreateEmbed::default()
        .title("✅ Queue Command Enabled")
        .description("Members can once again use `/queue` to manage their suggestions.")
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "status",
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "View whether the /queue command is enabled (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn queue_status(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;
    let enabled = queue_commands_enabled(pool, guild_id.get() as i64).await?;

    let embed = if enabled {
        CreateEmbed::default()
            .title("Queue Command Enabled")
            .description("Members can use `/queue` to view and manage their suggestions.")
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new(
                "Use `/config queue disable` to lock it down",
            ))
    } else {
        CreateEmbed::default()
            .title("Queue Command Disabled")
            .description(
                "Only `/adminqueue` is available. Use `/config queue enable` to restore access.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Configured via /config queue"))
    };

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    subcommands("pinning_disable", "pinning_enable", "pinning_status"),
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Control whether /finishbook and /select messages are pinned (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn pinning(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "disable",
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Stop pinning poll and announcement messages (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn pinning_disable(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;
    let guild_name = get_guild_name(&ctx).await;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    if !pin_polls_enabled(pool, guild_id.get() as i64).await? {
        let embed = CreateEmbed::default()
            .title("Pinning Already Disabled")
            .description(
                "Polls from `/finishbook` and `/select poll`, along with `/select` announcement messages, aren't being pinned right now.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    sqlx::query!(
        "INSERT INTO server_bot_config (server_id, pin_polls)
         VALUES ($1, $2)
         ON CONFLICT (server_id)
         DO UPDATE SET pin_polls = $2, updated_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64,
        false
    )
    .execute(pool)
    .await?;

    let embed = CreateEmbed::default()
        .title("Pinning Disabled")
        .description(
            "New polls and announcements from `/finishbook` and `/select` will no longer be pinned automatically.",
        )
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "enable",
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Let the bot pin poll and /select announcements (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn pinning_enable(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;
    let guild_name = get_guild_name(&ctx).await;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    if pin_polls_enabled(pool, guild_id.get() as i64).await? {
        let embed = CreateEmbed::default()
            .title("Pinning Already Enabled")
            .description(
                "`/finishbook` and `/select` polls nad announcements are already being pinned automatically when created.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    sqlx::query!(
        "INSERT INTO server_bot_config (server_id, pin_polls)
         VALUES ($1, $2)
         ON CONFLICT (server_id)
         DO UPDATE SET pin_polls = $2, updated_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64,
        true
    )
    .execute(pool)
    .await?;

    let embed = CreateEmbed::default()
        .title("Pinning Enabled")
        .description(
            "I'll pin new polls and announcements from `/finishbook` and `/select` automatically.",
        )
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Fumiko Book Club Bot"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "status",
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "See whether polls and /select announcements are pinned (requires Manage Server)",
    ),
    user_cooldown = 10
)]
async fn pinning_status(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let pool = &ctx.data().database;
    let enabled = pin_polls_enabled(pool, guild_id.get() as i64).await?;

    let embed = if enabled {
        CreateEmbed::default()
            .title("Pinning Enabled")
            .description(
                "`/finishbook` and `/select` polls and announcements are currently pinned automatically.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new(
                "Use `/config pinning disable` to stop pinning",
            ))
    } else {
        CreateEmbed::default()
            .title("Pinning Disabled")
            .description(
                "`/finishbook` and `/select` polls and announcements will stay unpinned until you re-enable it.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Use `/config pinning enable` to resume"))
    };

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    subcommands("mature_enable", "mature_disable", "mature_status"),
    guild_only,
    required_permissions = "ADMINISTRATOR",
    description_localized(
        "en-US",
        "Manage mature content settings for this server (requires Administrator)",
    ),
    user_cooldown = 10
)]
async fn mature(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "enable",
    guild_only,
    required_permissions = "ADMINISTRATOR",
    description_localized(
        "en-US",
        "Enable mature content for this server (requires Administrator)",
    ),
    user_cooldown = 10
)]
async fn mature_enable(ctx: Context<'_>) -> Result<(), Error> {
    super::mature::enable(ctx).await
}

#[poise::command(
    slash_command,
    rename = "disable",
    guild_only,
    required_permissions = "ADMINISTRATOR",
    description_localized(
        "en-US",
        "Disable mature content for this server (requires Administrator)",
    ),
    user_cooldown = 10
)]
async fn mature_disable(ctx: Context<'_>) -> Result<(), Error> {
    super::mature::disable(ctx).await
}

#[poise::command(
    slash_command,
    rename = "status",
    guild_only,
    required_permissions = "ADMINISTRATOR",
    description_localized(
        "en-US",
        "Check mature content settings for this server (requires Administrator)",
    ),
    user_cooldown = 10
)]
async fn mature_status(ctx: Context<'_>) -> Result<(), Error> {
    super::mature::status(ctx).await
}
