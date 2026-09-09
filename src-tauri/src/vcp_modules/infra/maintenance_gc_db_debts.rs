use super::{UnlinkDebt, UnlinkDebtCursor, UnlinkDebtPage, UNLINK_OUTBOX_PAGE_SIZE};

type DebtRow = (String, String, i64);

const CURSOR_BEFORE_SQL: &str = "SELECT root_kind, relative_path, created_at
             FROM attachment_gc_unlink_outbox
             WHERE created_at < ?
               AND (created_at > ?
                OR (created_at = ? AND root_kind > ?)
                OR (created_at = ? AND root_kind = ? AND relative_path > ?))
             ORDER BY created_at ASC, root_kind ASC, relative_path ASC
             LIMIT ?";
const CREATED_BEFORE_SQL: &str = "SELECT root_kind, relative_path, created_at
             FROM attachment_gc_unlink_outbox
             WHERE created_at < ?
             ORDER BY created_at ASC, root_kind ASC, relative_path ASC
             LIMIT ?";
const AFTER_CURSOR_SQL: &str = "SELECT root_kind, relative_path, created_at
             FROM attachment_gc_unlink_outbox
             WHERE created_at > ?
                OR (created_at = ? AND root_kind > ?)
                OR (created_at = ? AND root_kind = ? AND relative_path > ?)
             ORDER BY created_at ASC, root_kind ASC, relative_path ASC
             LIMIT ?";
const ALL_DEBTS_SQL: &str = "SELECT root_kind, relative_path, created_at
             FROM attachment_gc_unlink_outbox
             ORDER BY created_at ASC, root_kind ASC, relative_path ASC
             LIMIT ?";

pub(crate) async fn load_unlink_debts(
    connection: &mut sqlx::SqliteConnection,
    cursor: Option<&UnlinkDebtCursor>,
    created_before: Option<i64>,
) -> Result<UnlinkDebtPage, String> {
    let rows = load_debt_rows(connection, cursor, created_before)
        .await
        .map_err(|error| format!("读取附件物理 unlink 债务失败: {error}"))?;
    Ok(decode_debt_page(rows))
}

async fn load_debt_rows(
    connection: &mut sqlx::SqliteConnection,
    cursor: Option<&UnlinkDebtCursor>,
    created_before: Option<i64>,
) -> Result<Vec<DebtRow>, sqlx::Error> {
    match (cursor, created_before) {
        (Some(cursor), Some(created_before)) => {
            load_cursor_before(connection, cursor, created_before).await
        }
        (None, Some(created_before)) => load_created_before(connection, created_before).await,
        (Some(cursor), None) => load_after_cursor(connection, cursor).await,
        (None, None) => load_all_debts(connection).await,
    }
}

async fn load_cursor_before(
    connection: &mut sqlx::SqliteConnection,
    cursor: &UnlinkDebtCursor,
    created_before: i64,
) -> Result<Vec<DebtRow>, sqlx::Error> {
    sqlx::query_as::<_, DebtRow>(CURSOR_BEFORE_SQL)
        .bind(created_before)
        .bind(cursor.created_at)
        .bind(cursor.created_at)
        .bind(&cursor.root_kind)
        .bind(cursor.created_at)
        .bind(&cursor.root_kind)
        .bind(&cursor.relative_path)
        .bind(UNLINK_OUTBOX_PAGE_SIZE + 1)
        .fetch_all(&mut *connection)
        .await
}

async fn load_created_before(
    connection: &mut sqlx::SqliteConnection,
    created_before: i64,
) -> Result<Vec<DebtRow>, sqlx::Error> {
    sqlx::query_as::<_, DebtRow>(CREATED_BEFORE_SQL)
        .bind(created_before)
        .bind(UNLINK_OUTBOX_PAGE_SIZE + 1)
        .fetch_all(&mut *connection)
        .await
}

async fn load_after_cursor(
    connection: &mut sqlx::SqliteConnection,
    cursor: &UnlinkDebtCursor,
) -> Result<Vec<DebtRow>, sqlx::Error> {
    sqlx::query_as::<_, DebtRow>(AFTER_CURSOR_SQL)
        .bind(cursor.created_at)
        .bind(cursor.created_at)
        .bind(&cursor.root_kind)
        .bind(cursor.created_at)
        .bind(&cursor.root_kind)
        .bind(&cursor.relative_path)
        .bind(UNLINK_OUTBOX_PAGE_SIZE + 1)
        .fetch_all(&mut *connection)
        .await
}

async fn load_all_debts(
    connection: &mut sqlx::SqliteConnection,
) -> Result<Vec<DebtRow>, sqlx::Error> {
    sqlx::query_as::<_, DebtRow>(ALL_DEBTS_SQL)
        .bind(UNLINK_OUTBOX_PAGE_SIZE + 1)
        .fetch_all(&mut *connection)
        .await
}

fn decode_debt_page(mut rows: Vec<DebtRow>) -> UnlinkDebtPage {
    let has_more = rows.len() > UNLINK_OUTBOX_PAGE_SIZE as usize;
    rows.truncate(UNLINK_OUTBOX_PAGE_SIZE as usize);
    let next_cursor = rows
        .last()
        .map(|(root_kind, relative_path, created_at)| UnlinkDebtCursor {
            created_at: *created_at,
            root_kind: root_kind.clone(),
            relative_path: relative_path.clone(),
        });
    UnlinkDebtPage {
        has_more,
        next_cursor,
        debts: rows
            .into_iter()
            .map(|(root_kind, relative_path, created_at)| UnlinkDebt {
                root_kind,
                relative_path,
                created_at,
            })
            .collect(),
    }
}
