use std::sync::Arc;

use arrow_array::types::Float32Type;
use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Float64Array, Int64Array, RecordBatch,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};

use crate::StoreError;

use super::{ProjectNoteVectorRecord, VectorRecord};

fn vector_schema(embedding_dim: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("symbol_id", DataType::Utf8, false),
        Field::new("sir_hash", DataType::Utf8, false),
        Field::new("provider", DataType::Utf8, false),
        Field::new("model", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                embedding_dim,
            ),
            true,
        ),
        Field::new("updated_at", DataType::Int64, false),
    ]))
}

fn project_note_vector_schema(embedding_dim: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("note_id", DataType::Utf8, false),
        Field::new("provider", DataType::Utf8, false),
        Field::new("model", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                embedding_dim,
            ),
            true,
        ),
        Field::new("content", DataType::Utf8, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
    ]))
}

pub(super) fn single_record_batch(
    record: &VectorRecord,
) -> Result<(SchemaRef, RecordBatch), StoreError> {
    record_batch(std::slice::from_ref(record))
}

pub(super) fn record_batch(
    records: &[VectorRecord],
) -> Result<(SchemaRef, RecordBatch), StoreError> {
    if records.is_empty() {
        return Err(StoreError::LanceDb(
            "cannot build LanceDB record batch from empty records".to_owned(),
        ));
    }

    let embedding_dim = records[0].embedding.len() as i32;
    if embedding_dim <= 0 {
        return Err(StoreError::LanceDb(
            "embedding cannot be empty for LanceDB upsert".to_owned(),
        ));
    }
    if records
        .iter()
        .any(|record| record.embedding.len() as i32 != embedding_dim)
    {
        return Err(StoreError::LanceDb(
            "embedding dimensions must match within a LanceDB record batch".to_owned(),
        ));
    }

    let schema = vector_schema(embedding_dim);
    let embedding = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        records.iter().map(|record| {
            Some(
                record
                    .embedding
                    .iter()
                    .copied()
                    .map(Some)
                    .collect::<Vec<Option<f32>>>(),
            )
        }),
        embedding_dim,
    );

    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(
            records
                .iter()
                .map(|record| record.symbol_id.clone())
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            records
                .iter()
                .map(|record| record.sir_hash.clone())
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            records
                .iter()
                .map(|record| record.provider.clone())
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            records
                .iter()
                .map(|record| record.model.clone())
                .collect::<Vec<_>>(),
        )),
        Arc::new(embedding),
        Arc::new(Int64Array::from(
            records
                .iter()
                .map(|record| record.updated_at)
                .collect::<Vec<_>>(),
        )),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns)
        .map_err(|err| StoreError::LanceDb(err.to_string()))?;
    Ok((schema, batch))
}

pub(super) fn single_project_note_record_batch(
    record: &ProjectNoteVectorRecord,
) -> Result<(SchemaRef, RecordBatch), StoreError> {
    let embedding_dim = record.embedding.len() as i32;
    if embedding_dim <= 0 {
        return Err(StoreError::LanceDb(
            "embedding cannot be empty for LanceDB upsert".to_owned(),
        ));
    }

    let schema = project_note_vector_schema(embedding_dim);
    let embedding = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        std::iter::once(Some(
            record
                .embedding
                .iter()
                .copied()
                .map(Some)
                .collect::<Vec<Option<f32>>>(),
        )),
        embedding_dim,
    );

    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![record.note_id.clone()])),
        Arc::new(StringArray::from(vec![record.provider.clone()])),
        Arc::new(StringArray::from(vec![record.model.clone()])),
        Arc::new(embedding),
        Arc::new(StringArray::from(vec![record.content.clone()])),
        Arc::new(Int64Array::from(vec![record.created_at])),
        Arc::new(Int64Array::from(vec![record.updated_at])),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns)
        .map_err(|err| StoreError::LanceDb(err.to_string()))?;
    Ok((schema, batch))
}

pub(super) fn embedding_dim_from_schema(schema: &Schema) -> Option<i32> {
    let field = schema.field_with_name("embedding").ok()?;
    match field.data_type() {
        DataType::FixedSizeList(_, dim) => Some(*dim),
        _ => None,
    }
}

pub(super) fn distance_at(column: &ArrayRef, index: usize) -> Result<f32, StoreError> {
    if let Some(values) = column.as_any().downcast_ref::<Float32Array>() {
        if values.is_null(index) {
            return Err(StoreError::LanceDb("null distance value".to_owned()));
        }
        return Ok(values.value(index));
    }
    if let Some(values) = column.as_any().downcast_ref::<Float64Array>() {
        if values.is_null(index) {
            return Err(StoreError::LanceDb("null distance value".to_owned()));
        }
        return Ok(values.value(index) as f32);
    }

    Err(StoreError::LanceDb(format!(
        "unsupported _distance type: {:?}",
        column.data_type()
    )))
}

pub(super) fn string_at(
    batch: &RecordBatch,
    column_name: &str,
    row: usize,
) -> Result<String, StoreError> {
    let array = batch
        .column_by_name(column_name)
        .ok_or_else(|| StoreError::LanceDb(format!("missing column {column_name}")))?
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| StoreError::LanceDb(format!("column {column_name} is not Utf8")))?;
    if array.is_null(row) {
        return Err(StoreError::LanceDb(format!(
            "column {column_name} has null at row {row}"
        )));
    }
    Ok(array.value(row).to_owned())
}

pub(super) fn int64_at(
    batch: &RecordBatch,
    column_name: &str,
    row: usize,
) -> Result<i64, StoreError> {
    let array = batch
        .column_by_name(column_name)
        .ok_or_else(|| StoreError::LanceDb(format!("missing column {column_name}")))?
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| StoreError::LanceDb(format!("column {column_name} is not Int64")))?;
    if array.is_null(row) {
        return Err(StoreError::LanceDb(format!(
            "column {column_name} has null at row {row}"
        )));
    }
    Ok(array.value(row))
}

pub(super) fn embedding_at(
    batch: &RecordBatch,
    column_name: &str,
    row: usize,
) -> Result<Vec<f32>, StoreError> {
    let array = batch
        .column_by_name(column_name)
        .ok_or_else(|| StoreError::LanceDb(format!("missing column {column_name}")))?
        .as_any()
        .downcast_ref::<FixedSizeListArray>()
        .ok_or_else(|| StoreError::LanceDb(format!("column {column_name} is not FixedSizeList")))?;

    if array.is_null(row) {
        return Ok(Vec::new());
    }

    let values = array.value(row);
    let values = values
        .as_any()
        .downcast_ref::<Float32Array>()
        .ok_or_else(|| {
            StoreError::LanceDb(format!("column {column_name} values are not Float32"))
        })?;

    let mut embedding = Vec::with_capacity(values.len());
    for idx in 0..values.len() {
        if values.is_null(idx) {
            return Err(StoreError::LanceDb(format!(
                "column {column_name} has null embedding value"
            )));
        }
        embedding.push(values.value(idx));
    }
    Ok(embedding)
}
