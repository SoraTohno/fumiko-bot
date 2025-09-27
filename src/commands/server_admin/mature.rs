use crate::ensure_server_exists;
use crate::util::get_guild_name;
use crate::{types::Context, types::Error};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter};

const RESPONSIBILITY_DISCLAIMER: &str = "⚠️ **Disclaimer:** The onus is on server administrators and members to ensure the maturity toggle is used responsibly and remains compliant with Discord's Terms of Service and Community Guidelines.";

pub async fn enable(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };
    let guild_name = get_guild_name(&ctx).await;

    ensure_server_exists(pool, guild_id, &guild_name).await?;

    // Update or insert maturity setting
    sqlx::query!(
        "INSERT INTO server_maturity_settings (server_id, mature_content_enabled)
         VALUES ($1, true)
         ON CONFLICT (server_id) 
         DO UPDATE SET mature_content_enabled = true, updated_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64
    )
    .execute(pool)
    .await?;

    let embed = CreateEmbed::default()
        .title("✅ Mature Content Enabled")
        .description(format!(
            "Mature content has been enabled for this server.\n\n\
                 **Important:** Books marked as mature by Google Books will now be:\n\
                 • Visible in search results\n\
                 • Can be added to queues and lists\n\
                 • Displayable in any channel marked as NSFW (18+)\n\n{}",
            RESPONSIBILITY_DISCLAIMER
        ))
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Powered by Google Books API"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

pub async fn disable(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };
    let guild_name = get_guild_name(&ctx).await;

    ensure_server_exists(pool, guild_id, &guild_name).await?;

    // Update or insert maturity setting
    sqlx::query!(
        "INSERT INTO server_maturity_settings (server_id, mature_content_enabled)
         VALUES ($1, false)
         ON CONFLICT (server_id) 
         DO UPDATE SET mature_content_enabled = false, updated_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64
    )
    .execute(pool)
    .await?;

    let embed = CreateEmbed::default()
        .title("Mature Content Disabled")
        .description(format!(
            "Mature content has been disabled for this server.\n\n\
                 Books marked as mature by Google Books will no longer be:\n\
                 • Visible in search results\n\
                 • Addable to queues or lists\n\
                 • Displayable in any channel\n\n{}",
            RESPONSIBILITY_DISCLAIMER
        ))
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Powered by Google Books API"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

pub async fn status(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let setting = sqlx::query!(
        "SELECT mature_content_enabled FROM server_maturity_settings WHERE server_id = $1",
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    let is_enabled = setting.map(|s| s.mature_content_enabled).unwrap_or(false);

    let embed = if is_enabled {
        CreateEmbed::default()
            .title("Mature Content Status: Enabled")
            .description(format!(
                "Mature content is **enabled** for this server.\n\n\
                     Mature books can be displayed in channels marked as NSFW (18+).\n\
                     An administrator can disable this with `/config mature disable`.\n\n{}",
                RESPONSIBILITY_DISCLAIMER
            ))
            .color(0xB76E79)
    } else {
        CreateEmbed::default()
            .title("Mature Content Status: Disabled")
            .description(format!(
                "Mature content is **disabled** for this server.\n\n\
                     Books marked as mature will not be displayed.\n\
                     An administrator can enable this with `/config mature enable`.\n\n{}",
                RESPONSIBILITY_DISCLAIMER
            ))
            .color(0xB76E79)
    }
    .footer(CreateEmbedFooter::new("Powered by Google Books API"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}
