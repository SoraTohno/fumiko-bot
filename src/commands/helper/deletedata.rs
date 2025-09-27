use crate::util::get_guild_name;
use crate::{types::Context, types::Error};
use poise::CreateReply;
use poise::serenity_prelude::{
    ButtonStyle, ComponentInteraction, CreateActionRow, CreateButton, CreateEmbed,
};

#[poise::command(
    slash_command,
    subcommands("myself", "server"),
    description_localized("en-US", "Delete data from the bot"),
    user_cooldown = 10
)]
pub async fn deletedata(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "self",
    description_localized("en-US", "Delete all your personal data"),
    user_cooldown = 10
)]
async fn myself(ctx: Context<'_>) -> Result<(), Error> {
    // Create confirmation embed
    let embed = CreateEmbed::default()
        .title("⚠️ Delete Personal Data")
        .description(
            "This will permanently delete ALL your data including:\n\
            • Favorite books and authors\n\
            • Reading list\n\
            • All ratings and progress\n\n\
            **This action cannot be undone!**",
        )
        .color(0xFF0000);

    // Create confirmation buttons
    let confirm_button = CreateButton::new("confirm_delete_self")
        .label("Yes, delete my data")
        .style(ButtonStyle::Danger);

    let cancel_button = CreateButton::new("cancel_delete")
        .label("Cancel")
        .style(ButtonStyle::Secondary);

    let action_row = CreateActionRow::Buttons(vec![confirm_button, cancel_button]);

    let reply = CreateReply::default()
        .embed(embed)
        .components(vec![action_row]);

    let response = ctx.send(reply).await?;

    // Wait for button interaction
    let interaction = response
        .message()
        .await?
        .await_component_interaction(ctx.serenity_context())
        .author_id(ctx.author().id)
        .timeout(std::time::Duration::from_secs(60))
        .await;

    match interaction {
        Some(interaction) => {
            handle_delete_interaction(ctx, interaction, false).await?;
        }
        None => {
            ctx.send(
                CreateReply::default()
                    .content("Data deletion cancelled (timed out).")
                    .ephemeral(true),
            )
            .await?;
        }
    }

    Ok(())
}

#[poise::command(
    slash_command,
    rename = "server",
    guild_only,
    required_permissions = "ADMINISTRATOR",
    description_localized("en-US", "Delete all server data (requires Administrator)",),
    user_cooldown = 10
)]
async fn server(ctx: Context<'_>) -> Result<(), Error> {
    let Some(_guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(CreateReply::default().embed(embed)).await?;
        return Ok(());
    };
    let guild_name = get_guild_name(&ctx).await;

    // Create confirmation embed
    let embed = CreateEmbed::default()
        .title("⚠️ Delete Server Data")
        .description(format!(
            "This will permanently delete ALL data for **{}** including:\n\
            • Book queue\n\
            • Current and completed books\n\
            • All member ratings and progress\n\
            • Server configuration\n\n\
            **This action cannot be undone and will affect all members!**",
            guild_name
        ))
        .color(0xFF0000);

    // Create confirmation buttons
    let confirm_button = CreateButton::new("confirm_delete_server")
        .label("Yes, delete all server data")
        .style(ButtonStyle::Danger);

    let cancel_button = CreateButton::new("cancel_delete")
        .label("Cancel")
        .style(ButtonStyle::Secondary);

    let action_row = CreateActionRow::Buttons(vec![confirm_button, cancel_button]);

    let reply = CreateReply::default()
        .embed(embed)
        .components(vec![action_row]);

    let response = ctx.send(reply).await?;

    // Wait for button interaction
    let interaction = response
        .message()
        .await?
        .await_component_interaction(ctx.serenity_context())
        .author_id(ctx.author().id)
        .timeout(std::time::Duration::from_secs(60))
        .await;

    match interaction {
        Some(interaction) => {
            handle_delete_interaction(ctx, interaction, true).await?;
        }
        None => {
            ctx.send(
                CreateReply::default()
                    .content("Server data deletion cancelled (timed out).")
                    .ephemeral(true),
            )
            .await?;
        }
    }

    Ok(())
}

async fn handle_delete_interaction(
    ctx: Context<'_>,
    interaction: ComponentInteraction,
    is_server: bool,
) -> Result<(), Error> {
    interaction.defer(&ctx.http()).await?;

    match interaction.data.custom_id.as_str() {
        "confirm_delete_self" => {
            let pool = &ctx.data().database;

            // Use the database function to delete user data
            sqlx::query!("SELECT delete_user_data($1)", ctx.author().id.get() as i64)
                .execute(pool)
                .await?;

            interaction
                .edit_response(
                    &ctx.http(),
                    poise::serenity_prelude::EditInteractionResponse::new()
                        .content("✅ Your personal data has been permanently deleted.")
                        .components(vec![]),
                )
                .await?;
        }
        "confirm_delete_server" => {
            let pool = &ctx.data().database;
            let Some(guild_id) = ctx.guild_id() else {
                interaction
                    .edit_response(
                        &ctx.http(),
                        poise::serenity_prelude::EditInteractionResponse::new()
                            .content("❌ Couldn't determine which server to delete. Try again from within a server.")
                            .components(vec![]),
                    )
                    .await?;
                return Ok(());
            };

            // Use the database function to delete server data
            sqlx::query!("SELECT delete_server_data($1)", guild_id.get() as i64)
                .execute(pool)
                .await?;

            interaction
                .edit_response(
                    &ctx.http(),
                    poise::serenity_prelude::EditInteractionResponse::new()
                        .content("✅ All server data has been permanently deleted.")
                        .components(vec![]),
                )
                .await?;
        }
        "cancel_delete" => {
            let msg = if is_server {
                "Server data deletion cancelled."
            } else {
                "Personal data deletion cancelled."
            };

            interaction
                .edit_response(
                    &ctx.http(),
                    poise::serenity_prelude::EditInteractionResponse::new()
                        .content(msg)
                        .components(vec![]),
                )
                .await?;
        }
        _ => {}
    }

    Ok(())
}
