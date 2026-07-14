//! memory_store repo - 长期记忆存储库的 CRUD 操作。
use rusqlite::Connection;

use crate::error::AppResult;

#[derive(Debug, Clone)]
pub struct NewMemory {
    pub id: String,
    pub agent_id: String,
    pub content: String,
    pub memory_type: String, // "Preference" | "Fact" | "Context" | "Codebase"
    pub scope: String,       // "global" | "agent"
    pub source: String,
    pub confidence: f64,
    pub embedding_id: Option<String>,
}

/// 混合检索：sqlite-vec 向量 KNN + LIKE 子串精确匹配融合，限定 agent_id。
pub fn search(
    conn: &Connection,
    query_text: &str,
    query_vector: Option<&[f32]>,
    agent_id: &str,
) -> AppResult<Vec<String>> {
    // 1. 基于 LIKE 的文本检索
    let mut stmt_text = conn.prepare(
        "SELECT content FROM memory_store \
         WHERE agent_id = ?1 AND status = 'active' AND content LIKE ?2 \
         LIMIT 10",
    )?;
    let like_pattern = format!("%{}%", query_text);
    let mut text_rows = stmt_text.query([agent_id, &like_pattern])?;
    let mut text_results = Vec::new();
    while let Some(row) = text_rows.next()? {
        let content: String = row.get(0)?;
        text_results.push(content);
    }

    // 2. 基于 sqlite-vec 的向量检索
    let mut vector_results = Vec::new();
    if let Some(vec) = query_vector {
        let vec_bytes = f32_to_bytes(vec);
        let mut stmt_vec = conn.prepare(
            "SELECT m.content FROM vec_embeddings v \
             JOIN memory_store m ON m.embedding_id = v.embedding_id \
             WHERE v.vector MATCH ?1 AND k = 10 AND m.agent_id = ?2 AND m.status = 'active'",
        )?;
        let mut vec_rows = stmt_vec.query((vec_bytes, agent_id))?;
        while let Some(row) = vec_rows.next()? {
            let content: String = row.get(0)?;
            vector_results.push(content);
        }
    }

    // 3. 去重合并检索结果
    let mut combined = text_results;
    for val in vector_results {
        if !combined.contains(&val) {
            combined.push(val);
        }
    }

    Ok(combined)
}

/// 插入一条新的长期记忆条目。
pub fn insert(conn: &Connection, m: &NewMemory) -> AppResult<()> {
    let now_str = chrono_like_now();
    conn.execute(
        "INSERT INTO memory_store (id, agent_id, content, type, scope, source, confidence, created_at, updated_at, embedding_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        (
            &m.id,
            &m.agent_id,
            &m.content,
            &m.memory_type,
            &m.scope,
            &m.source,
            m.confidence,
            &now_str,
            &now_str,
            &m.embedding_id,
        ),
    )?;
    Ok(())
}

/// 插入一条向量及元数据关联项。
pub fn insert_embedding(
    conn: &Connection,
    embedding_id: &str,
    ref_type: &str,
    ref_id: &str,
    model: &str,
    dims: i32,
    content_hash: &str,
    vector: &[f32],
) -> AppResult<()> {
    let now_str = chrono_like_now();
    // 1. 写入元数据
    conn.execute(
        "INSERT INTO embedding_items (id, ref_type, ref_id, model, dims, content_hash, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            embedding_id,
            ref_type,
            ref_id,
            model,
            dims,
            content_hash,
            &now_str,
        ),
    )?;

    // 2. 写入 sqlite-vec 虚拟表
    let vec_bytes = f32_to_bytes(vector);
    conn.execute(
        "INSERT INTO vec_embeddings (embedding_id, vector) VALUES (?1, ?2)",
        (embedding_id, vec_bytes),
    )?;

    Ok(())
}

fn f32_to_bytes(v: &[f32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            v.as_ptr() as *const u8,
            v.len() * std::mem::size_of::<f32>(),
        )
    }
}

fn chrono_like_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}
