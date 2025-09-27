-- Discord Users table
CREATE TABLE public.discord_users (
    user_id BIGINT PRIMARY KEY, -- Discord user ID (snowflake)
    username TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

-- Discord Servers/Guilds table
CREATE TABLE public.discord_servers (
    server_id BIGINT PRIMARY KEY, -- Discord server/guild ID (snowflake)
    server_name TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

-- User Favorite Books (stores only Google Books volume IDs)
CREATE TABLE public.user_favorite_books (
    user_id BIGINT NOT NULL REFERENCES discord_users(user_id) ON DELETE CASCADE,
    server_id BIGINT NOT NULL REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    volume_id TEXT NOT NULL, -- Google Books volume ID only
    added_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    is_number_one BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (user_id, server_id, volume_id)
);

-- User Favorite Authors (user-provided names, not from API)
-- CREATE TABLE public.user_favorite_authors (
--     user_id BIGINT NOT NULL REFERENCES discord_users(user_id) ON DELETE CASCADE,
--     server_id BIGINT NOT NULL REFERENCES discord_servers(server_id) ON DELETE CASCADE,
--     author_name TEXT NOT NULL, -- User-provided author name
--     added_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
--     is_number_one BOOLEAN NOT NULL DEFAULT FALSE,
--     PRIMARY KEY (user_id, server_id, author_name)
-- );

-- User Reading Progress (current book progress)
CREATE TABLE public.user_reading_progress (
    user_id BIGINT NOT NULL REFERENCES discord_users(user_id) ON DELETE CASCADE,
    server_id BIGINT NOT NULL REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    volume_id TEXT NOT NULL, -- Google Books volume ID only
    progress_text TEXT,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, server_id)
);

-- Progress command bans
CREATE TABLE public.progress_command_bans (
    server_id BIGINT NOT NULL REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL REFERENCES discord_users(user_id) ON DELETE CASCADE,
    banned_by BIGINT REFERENCES discord_users(user_id) ON DELETE SET NULL,
    banned_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (server_id, user_id)
);

-- User Reading List (books to read in the future)
CREATE TABLE public.user_reading_list (
    user_id BIGINT NOT NULL REFERENCES discord_users(user_id) ON DELETE CASCADE,
    server_id BIGINT NOT NULL REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    volume_id TEXT NOT NULL, -- Google Books volume ID only
    added_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, server_id, volume_id)
);

-- Server Book Queue
CREATE TABLE public.server_book_queue (
    queue_id SERIAL PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    volume_id TEXT NOT NULL, -- Google Books volume ID only
    suggested_by_user_id BIGINT NOT NULL REFERENCES discord_users(user_id) ON DELETE CASCADE,
    added_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    position INTEGER NOT NULL,
    UNIQUE (server_id, volume_id),
    -- UNIQUE (server_id, suggested_by_user_id) -- One book per person in queue, not currently in use due to adminqueue not liking it for current impl
);

-- Server Current Book (UPDATED: Added suggested_by_user_id)
CREATE TABLE public.server_current_book (
    server_id BIGINT PRIMARY KEY REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    volume_id TEXT NOT NULL, -- Google Books volume ID only
    suggested_by_user_id BIGINT REFERENCES discord_users(user_id) ON DELETE SET NULL,
    started_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    deadline TIMESTAMP WITH TIME ZONE,
    announcement_channel_id BIGINT,
    discussion_thread_id BIGINT
);

-- Server Completed Books (UPDATED: Added suggested_by_user_id)
CREATE TABLE public.server_completed_books (
    completed_id SERIAL PRIMARY KEY,
    server_id BIGINT NOT NULL REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    volume_id TEXT NOT NULL, -- Google Books volume ID only
    suggested_by_user_id BIGINT REFERENCES discord_users(user_id) ON DELETE SET NULL,
    started_at TIMESTAMP WITH TIME ZONE NOT NULL,
    completed_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    average_rating DECIMAL(3,2),
    total_ratings INTEGER DEFAULT 0,
    UNIQUE (server_id, volume_id, completed_at)
);

-- User Book Ratings (for completed books)
CREATE TABLE public.user_book_ratings (
    user_id BIGINT NOT NULL REFERENCES discord_users(user_id) ON DELETE CASCADE,
    completed_id INTEGER NOT NULL REFERENCES server_completed_books(completed_id) ON DELETE CASCADE,
    rating INTEGER NOT NULL CHECK (rating >= 1 AND rating <= 5),
    rated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, completed_id)
);

-- Bot Configuration per Server
CREATE TABLE public.server_bot_config (
    server_id BIGINT PRIMARY KEY REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    announcement_channel_id BIGINT,
    discussion_channel_id BIGINT,
    queue_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    pin_polls BOOLEAN NOT NULL DEFAULT TRUE,
    auto_complete_on_deadline BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

-- Server maturity settings
CREATE TABLE public.server_maturity_settings (
    server_id BIGINT PRIMARY KEY REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    mature_content_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

-- Poll tables
CREATE TABLE IF NOT EXISTS rating_polls (
    message_id BIGINT PRIMARY KEY,
    channel_id BIGINT NOT NULL,
    server_id BIGINT NOT NULL REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    completed_id INTEGER NOT NULL REFERENCES server_completed_books(completed_id) ON DELETE CASCADE,
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
    processed BOOLEAN DEFAULT FALSE,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS selection_polls (
    message_id BIGINT PRIMARY KEY,
    channel_id BIGINT NOT NULL,
    server_id BIGINT NOT NULL REFERENCES discord_servers(server_id) ON DELETE CASCADE,
    book_options TEXT[] NOT NULL, -- Array of volume_ids
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
    processed BOOLEAN DEFAULT FALSE,
    selected_volume_id TEXT, -- The winning book's volume_id
    deadline TIMESTAMP WITH TIME ZONE,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

-- INDEXES

CREATE INDEX idx_user_favorite_books_user_id ON user_favorite_books(user_id); -- is this still needed?
-- CREATE INDEX idx_user_favorite_authors_user_id ON user_favorite_authors(user_id);
CREATE INDEX idx_user_reading_progress_server_id ON user_reading_progress(server_id);
CREATE INDEX idx_user_reading_list_user_id ON user_reading_list(user_id);
CREATE INDEX idx_server_book_queue_server_id ON server_book_queue(server_id);
CREATE INDEX idx_server_book_queue_position ON server_book_queue(server_id, position);
CREATE INDEX idx_server_completed_books_server_id ON server_completed_books(server_id);
CREATE INDEX idx_user_book_ratings_completed_id ON user_book_ratings(completed_id);
CREATE INDEX idx_rating_polls_expires_at ON rating_polls(expires_at) WHERE NOT processed;
CREATE INDEX idx_selection_polls_expires_at ON selection_polls(expires_at) WHERE NOT processed;
CREATE INDEX idx_server_maturity_enabled ON server_maturity_settings(server_id) WHERE mature_content_enabled;
CREATE INDEX idx_user_favorite_books_user_server ON user_favorite_books(user_id, server_id);
-- CREATE INDEX idx_user_favorite_authors_user_server ON user_favorite_authors(user_id, server_id);
CREATE INDEX idx_user_reading_list_user_server ON user_reading_list(user_id, server_id);
CREATE INDEX idx_progress_command_bans_user_id ON progress_command_bans(user_id);


-- Prevent more than one unprocessed selection poll per server
CREATE UNIQUE INDEX IF NOT EXISTS uidx_one_active_selection_poll
    ON selection_polls(server_id)
    WHERE NOT processed;

-- Unique indexes for "number one" items updated to be per server
CREATE UNIQUE INDEX uidx_user_one_fav_book_per_server
    ON public.user_favorite_books (user_id, server_id)
    WHERE is_number_one;

-- CREATE UNIQUE INDEX uidx_user_one_fav_author_per_server
--     ON public.user_favorite_authors (user_id, server_id)
--     WHERE is_number_one;

-- VIEWS

-- View for server book ratings (updated to not include title)
CREATE OR REPLACE VIEW server_book_ratings_view AS
SELECT 
    scb.server_id,
    scb.volume_id,
    ubr.user_id,
    ubr.rating,
    ubr.rated_at
FROM server_completed_books scb
JOIN user_book_ratings ubr ON ubr.completed_id = scb.completed_id;

-- TRIGGER FUNCTIONS

-- Trigger for updating average ratings
CREATE OR REPLACE FUNCTION update_average_rating()
RETURNS TRIGGER AS $$
DECLARE
    v_completed_id INTEGER := COALESCE(NEW.completed_id, OLD.completed_id);
BEGIN
    UPDATE server_completed_books
    SET average_rating = (
        SELECT AVG(rating)::DECIMAL(3,2)
        FROM user_book_ratings
        WHERE completed_id = v_completed_id
    ),
    total_ratings = (
        SELECT COUNT(*)
        FROM user_book_ratings
        WHERE completed_id = v_completed_id
    )
    WHERE completed_id = v_completed_id;

    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;

-- Function to maintain queue positions
CREATE OR REPLACE FUNCTION reorder_queue_positions()
RETURNS TRIGGER AS $$
BEGIN
    WITH numbered_queue AS (
        SELECT queue_id, 
               ROW_NUMBER() OVER (ORDER BY position, added_at) as new_position
        FROM server_book_queue
        WHERE server_id = OLD.server_id
    )
    UPDATE server_book_queue sq
    SET position = nq.new_position
    FROM numbered_queue nq
    WHERE sq.queue_id = nq.queue_id;
    
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Function to check user reading list limit (reduced to 5)
CREATE OR REPLACE FUNCTION check_reading_list_limit() 
RETURNS TRIGGER AS $$
BEGIN
    IF (SELECT COUNT(*) FROM user_reading_list WHERE user_id = NEW.user_id AND server_id = NEW.server_id) >= 5 THEN
        RAISE EXCEPTION 'User reading list cannot exceed 5 books per server';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Function to check user favorite books limit (reduced to 5)
CREATE OR REPLACE FUNCTION check_favorite_books_limit() 
RETURNS TRIGGER AS $$
BEGIN
    IF (SELECT COUNT(*) FROM user_favorite_books WHERE user_id = NEW.user_id AND server_id = NEW.server_id) >= 5 THEN
        RAISE EXCEPTION 'User favorite books cannot exceed 5 books per server';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Function to check user favorite authors limit (reduced to 5)
-- CREATE OR REPLACE FUNCTION check_favorite_authors_limit() 
-- RETURNS TRIGGER AS $$
-- BEGIN
--     IF (SELECT COUNT(*) FROM user_favorite_authors WHERE user_id = NEW.user_id AND server_id = NEW.server_id) >= 5 THEN
--         RAISE EXCEPTION 'User favorite authors cannot exceed 5 authors per server';
--     END IF;
--     RETURN NEW;
-- END;
-- $$ LANGUAGE plpgsql;

-- TRIGGERS

CREATE TRIGGER trigger_update_average_rating
AFTER INSERT OR UPDATE OR DELETE ON user_book_ratings
FOR EACH ROW
EXECUTE FUNCTION update_average_rating();

CREATE TRIGGER trigger_reorder_queue
AFTER DELETE ON server_book_queue
FOR EACH ROW
EXECUTE FUNCTION reorder_queue_positions();

CREATE TRIGGER enforce_reading_list_limit
BEFORE INSERT ON user_reading_list
FOR EACH ROW
EXECUTE FUNCTION check_reading_list_limit();

CREATE TRIGGER enforce_favorite_books_limit
BEFORE INSERT ON user_favorite_books
FOR EACH ROW
EXECUTE FUNCTION check_favorite_books_limit();

-- CREATE TRIGGER enforce_favorite_authors_limit
-- BEFORE INSERT ON user_favorite_authors
-- FOR EACH ROW
-- EXECUTE FUNCTION check_favorite_authors_limit();

-- BUSINESS LOGIC FUNCTIONS

-- Function to move a book from queue to current (UPDATED)
CREATE OR REPLACE FUNCTION select_book_from_queue_tx(
    p_server_id BIGINT,
    p_volume_id TEXT,
    p_announcement_channel_id BIGINT DEFAULT NULL,
    p_deadline TIMESTAMP WITH TIME ZONE DEFAULT NULL
)
RETURNS TABLE (
    volume_id TEXT,
    suggested_by_user_id BIGINT,
    suggested_by_username TEXT,
    success BOOLEAN,
    error_message TEXT
) AS $$
DECLARE
    v_current_book TEXT;
    v_book_exists BOOLEAN;
BEGIN
    -- Check if book exists in queue
    SELECT EXISTS(
        SELECT 1
        FROM server_book_queue sbq
        WHERE sbq.server_id = p_server_id
        AND sbq.volume_id = p_volume_id
    ) INTO v_book_exists;
    
    IF NOT v_book_exists THEN
        RETURN QUERY SELECT 
            NULL::TEXT, NULL::BIGINT, NULL::TEXT, 
            FALSE, 'Book not found in queue'::TEXT;
        RETURN;
    END IF;
    
    -- Check if there's already a current book
    SELECT scb.volume_id INTO v_current_book
    FROM server_current_book scb
    WHERE scb.server_id = p_server_id;
    
    IF v_current_book IS NOT NULL THEN
        RETURN QUERY SELECT 
            NULL::TEXT, NULL::BIGINT, NULL::TEXT,
            FALSE, 'Server already has a current book'::TEXT;
        RETURN;
    END IF;
    
    -- Get book details BEFORE removing from queue
    CREATE TEMP TABLE book_details ON COMMIT DROP AS
    SELECT 
        sbq.volume_id,
        sbq.suggested_by_user_id,
        du.username as suggested_by_username
    FROM server_book_queue sbq
    JOIN discord_users du ON du.user_id = sbq.suggested_by_user_id
    WHERE sbq.server_id = p_server_id 
    AND sbq.volume_id = p_volume_id;
    
    -- Insert into current book (UPDATED to include suggested_by_user_id)
    INSERT INTO server_current_book (
        server_id,
        volume_id,
        suggested_by_user_id,
        announcement_channel_id,
        deadline
    )
    SELECT p_server_id, bd.volume_id, bd.suggested_by_user_id, p_announcement_channel_id, p_deadline
    FROM book_details bd;
    
    -- Remove from queue
    DELETE FROM server_book_queue sbq
    WHERE sbq.server_id = p_server_id
        AND sbq.volume_id = p_volume_id;
    
    -- Return success with book details
    RETURN QUERY 
    SELECT bd.*, TRUE, NULL::TEXT
    FROM book_details bd;
    
EXCEPTION WHEN OTHERS THEN
    RETURN QUERY SELECT 
        NULL::TEXT, NULL::BIGINT, NULL::TEXT,
        FALSE, SQLERRM::TEXT;
END;
$$ LANGUAGE plpgsql;

-- Function to finish current book and move to completed (UPDATED)
CREATE OR REPLACE FUNCTION finish_current_book_tx(p_server_id BIGINT)
RETURNS TABLE (
    completed_id INTEGER,
    volume_id TEXT,
    started_at TIMESTAMP WITH TIME ZONE,
    success BOOLEAN,
    error_message TEXT
) AS $$
DECLARE
    v_current_book RECORD;
    v_completed_id INTEGER;
BEGIN
    -- Get current book with lock (UPDATED to include suggested_by_user_id)
    SELECT scb.volume_id, scb.started_at, scb.suggested_by_user_id
    INTO v_current_book
    FROM server_current_book scb
    WHERE scb.server_id = p_server_id
    FOR UPDATE;
    
    IF v_current_book.volume_id IS NULL THEN
        RETURN QUERY SELECT 
            NULL::INTEGER, NULL::TEXT, NULL::TIMESTAMP WITH TIME ZONE,
            FALSE, 'No current book to finish'::TEXT;
        RETURN;
    END IF;
    
    -- Move to completed books (UPDATED to include suggested_by_user_id)
    INSERT INTO server_completed_books (server_id, volume_id, suggested_by_user_id, started_at)
    VALUES (p_server_id, v_current_book.volume_id, v_current_book.suggested_by_user_id, v_current_book.started_at)
    RETURNING server_completed_books.completed_id INTO v_completed_id;

    -- Clear reading progress for all users in this server
    DELETE FROM user_reading_progress
    WHERE server_id = p_server_id;
    
    -- Remove current book
    DELETE FROM server_current_book
    WHERE server_id = p_server_id;
    
    -- Return success with completed book info
    RETURN QUERY
    SELECT 
        v_completed_id,
        scb.volume_id,
        scb.started_at,
        TRUE,
        NULL::TEXT
    FROM server_completed_books scb
    WHERE scb.completed_id = v_completed_id;
    
EXCEPTION WHEN OTHERS THEN
    RETURN QUERY SELECT 
        NULL::INTEGER, NULL::TEXT, NULL::TIMESTAMP WITH TIME ZONE,
        FALSE, SQLERRM::TEXT;
END;
$$ LANGUAGE plpgsql;

-- Function to get random book from queue
CREATE OR REPLACE FUNCTION get_random_queue_book(p_server_id BIGINT)
RETURNS TABLE (
    volume_id TEXT,
    suggested_by_username TEXT
) AS $$
BEGIN
    RETURN QUERY
    SELECT 
        sbq.volume_id,
        du.username as suggested_by_username
    FROM server_book_queue sbq
    JOIN discord_users du ON du.user_id = sbq.suggested_by_user_id
    WHERE sbq.server_id = p_server_id
    ORDER BY RANDOM()
    LIMIT 1;
END;
$$ LANGUAGE plpgsql;

-- Function to get books for poll
CREATE OR REPLACE FUNCTION get_queue_books_for_poll(
    p_server_id BIGINT,
    p_poll_size INTEGER DEFAULT 5
)
RETURNS TABLE (
    volume_id TEXT,
    suggested_by_username TEXT,
    "position" INTEGER
) AS $$
BEGIN
    RETURN QUERY
    SELECT 
        sbq.volume_id,
        du.username as suggested_by_username,
        sbq.position
    FROM server_book_queue sbq
    JOIN discord_users du ON du.user_id = sbq.suggested_by_user_id
    WHERE sbq.server_id = p_server_id
    ORDER BY sbq.position
    LIMIT p_poll_size;
END;
$$ LANGUAGE plpgsql;

-- Remove the current book without marking it completed (UPDATED)
CREATE OR REPLACE FUNCTION remove_current_book_tx(p_server_id BIGINT)
RETURNS TABLE (
    volume_id TEXT,
    success BOOLEAN,
    error_message TEXT
) AS $$
DECLARE
    v_volume_id TEXT;
BEGIN
    -- Lock current book row for this server
    SELECT scb.volume_id
    INTO v_volume_id
    FROM server_current_book scb
    WHERE scb.server_id = p_server_id
    FOR UPDATE;

    IF v_volume_id IS NULL THEN
        RETURN QUERY SELECT
            NULL::TEXT, FALSE, 'No current book to remove'::TEXT;
        RETURN;
    END IF;

    -- Clear per-server reading progress
    DELETE FROM user_reading_progress
    WHERE server_id = p_server_id;

    -- Remove the current book
    DELETE FROM server_current_book
    WHERE server_id = p_server_id;

    -- Return the removed book info
    RETURN QUERY SELECT
        v_volume_id,
        TRUE,
        NULL::TEXT;

EXCEPTION WHEN OTHERS THEN
    RETURN QUERY SELECT
        NULL::TEXT, FALSE, SQLERRM::TEXT;
END;
$$ LANGUAGE plpgsql;

-- Function to get server book rankings (UPDATED to include suggested_by) + (FIXED ranking logic)
CREATE OR REPLACE FUNCTION get_server_book_rankings(p_server_id BIGINT)
RETURNS TABLE (
    rank INTEGER,
    volume_id TEXT,
    suggested_by_username TEXT,
    average_rating DECIMAL(3,2),
    total_ratings INTEGER,
    completed_at TIMESTAMP WITH TIME ZONE
) AS $$
BEGIN
    RETURN QUERY
    SELECT 
        DENSE_RANK() OVER (ORDER BY scb.average_rating DESC NULLS LAST)::INTEGER as rank,
        scb.volume_id,
        du.username as suggested_by_username,
        scb.average_rating,
        scb.total_ratings,
        scb.completed_at
    FROM server_completed_books scb
    LEFT JOIN discord_users du ON du.user_id = scb.suggested_by_user_id
    WHERE scb.server_id = p_server_id
    ORDER BY scb.average_rating DESC NULLS LAST, scb.completed_at DESC;
END;
$$ LANGUAGE plpgsql;

-- Function to get users who favorited a book in a server
CREATE OR REPLACE FUNCTION get_book_favorites_in_server(
    p_volume_id TEXT,
    p_server_id BIGINT
)
RETURNS TABLE (
    user_id BIGINT,
    username TEXT
) AS $$
BEGIN
    RETURN QUERY
    SELECT DISTINCT
        ufb.user_id,
        du.username
    FROM user_favorite_books ufb
    JOIN discord_users du ON du.user_id = ufb.user_id
    WHERE ufb.volume_id = p_volume_id
    AND ufb.server_id = p_server_id
    ORDER BY du.username;
END;
$$ LANGUAGE plpgsql;

-- DATA DELETION FUNCTIONS

CREATE OR REPLACE FUNCTION delete_user_data(p_user_id BIGINT)
RETURNS VOID AS $$
BEGIN
    DELETE FROM discord_users WHERE user_id = p_user_id;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION delete_server_data(p_server_id BIGINT)
RETURNS VOID AS $$
BEGIN
    DELETE FROM discord_servers WHERE server_id = p_server_id;
END;
$$ LANGUAGE plpgsql;