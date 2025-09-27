use crate::google_books_cache::CachedGoogleBooksClient;
use sqlx::PgPool;
use std::collections::HashSet;

/// Pre-warm the cache with frequently accessed books
pub async fn warm_cache(pool: &PgPool, google_books: &CachedGoogleBooksClient) {
    println!("🔥 Starting cache pre-warming...");

    let mut volume_ids = HashSet::new();

    // Get all current books being read across all servers
    if let Ok(current_books) = sqlx::query!("SELECT DISTINCT volume_id FROM server_current_book")
        .fetch_all(pool)
        .await
    {
        for book in current_books {
            volume_ids.insert(book.volume_id);
        }
    }

    // Get recently queued books (top 5 from each server - reduced from 20)
    if let Ok(queued_books) = sqlx::query!(
        r#"
        SELECT DISTINCT volume_id 
        FROM (
            SELECT volume_id, 
                   ROW_NUMBER() OVER (PARTITION BY server_id ORDER BY position) as rn
            FROM server_book_queue
        ) ranked
        WHERE rn <= 5
        "#
    )
    .fetch_all(pool)
    .await
    {
        for book in queued_books {
            volume_ids.insert(book.volume_id);
        }
    }

    // Get recently completed books (last 3 per server - reduced from 10)
    if let Ok(completed_books) = sqlx::query!(
        r#"
        SELECT DISTINCT volume_id 
        FROM (
            SELECT volume_id,
                   ROW_NUMBER() OVER (PARTITION BY server_id ORDER BY completed_at DESC) as rn
            FROM server_completed_books
        ) ranked
        WHERE rn <= 3
        "#
    )
    .fetch_all(pool)
    .await
    {
        for book in completed_books {
            volume_ids.insert(book.volume_id);
        }
    }

    // Pre-fetch volumes in smaller batches
    let total = volume_ids.len();
    let mut success = 0;
    let mut errors = 0;

    // Convert to vector for batch processing
    let volume_vec: Vec<String> = volume_ids.into_iter().collect();

    // Process in batches of 5 to respect rate limits
    for chunk in volume_vec.chunks(5) {
        let results = google_books.get_volumes_batch(chunk).await;

        for result in results {
            match result {
                Ok(_) => success += 1,
                Err(e) => {
                    errors += 1;
                    eprintln!("Failed to pre-warm volume: {}", e);
                }
            }
        }

        // Add delay between batches to avoid rate limiting
        if chunk.len() == 5 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    }

    println!(
        "✅ Cache pre-warming complete: {}/{} volumes cached ({} errors)",
        success, total, errors
    );
}

/// Periodically refresh cache for active books
pub async fn start_cache_refresh_task(pool: PgPool, google_books: CachedGoogleBooksClient) {
    tokio::spawn(async move {
        // Initial warm-up after 30 seconds (increased from 10)
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
        warm_cache(&pool, &google_books).await;

        // Then refresh every 6 hours (increased from 2)
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(21600));
        loop {
            interval.tick().await;
            warm_cache(&pool, &google_books).await;
        }
    });
}
