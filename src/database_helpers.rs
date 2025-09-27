use crate::types::Error;
use sqlx::PgPool;

pub struct BookSelectionResult {
    pub volume_id: Option<String>,
    pub suggested_by_user_id: Option<i64>,
    pub suggested_by_username: Option<String>,
    pub success: Option<bool>,
    pub error_message: Option<String>,
}

pub struct BookCompletionResult {
    pub completed_id: Option<i32>,
    pub volume_id: Option<String>,
    pub started_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    pub success: Option<bool>,
    pub error_message: Option<String>,
}

pub async fn select_book_transactional(
    pool: &PgPool,
    guild_id: i64,
    volume_id: &str,
    announcement_channel_id: Option<i64>,
    deadline: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
) -> Result<BookSelectionResult, Error> {
    let result = sqlx::query_as!(
        BookSelectionResult,
        r#"
        SELECT
            volume_id,
            suggested_by_user_id,
            suggested_by_username,
            success,
            error_message
        FROM select_book_from_queue_tx($1, $2, $3, $4)
        "#,
        guild_id,
        volume_id,
        announcement_channel_id,
        deadline
    )
    .fetch_one(pool)
    .await?;

    if !result.success.unwrap_or(false) {
        return Err(result
            .error_message
            .unwrap_or_else(|| "Unknown error".to_string())
            .into());
    }

    Ok(result)
}

pub async fn finish_book_transactional(
    pool: &PgPool,
    guild_id: i64,
) -> Result<BookCompletionResult, Error> {
    let result = sqlx::query_as!(
        BookCompletionResult,
        r#"
        SELECT
            f.completed_id,
            f.volume_id,
            f.started_at,
            f.success,
            f.error_message
        FROM finish_current_book_tx($1) AS f
        "#,
        guild_id
    )
    .fetch_one(pool)
    .await?;

    if !result.success.unwrap_or(false) {
        return Err(result
            .error_message
            .unwrap_or_else(|| "Unknown error".to_string())
            .into());
    }

    Ok(result)
}
