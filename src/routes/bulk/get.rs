// Reacher - Email Verification
// Copyright (C) 2018-2022 Reacher

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! This file implements the `GET /bulk/{id}` endpoint.

use std::convert::{TryFrom, TryInto};

use crate::errors::ReacherError;

use csv::WriterBuilder;
use sqlx::{Executor, Pool, Postgres, Row};
use warp::Filter;

use serde::{Deserialize, Serialize};

use sqlx::types::chrono::{DateTime, Utc};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum JobResultResponseFormat {
	Json,
	Csv,
}

// limit and offset are optional in the request
// if they are unspecified their default values
// are 50 and 0 respectively
#[derive(Serialize, Deserialize)]
struct JobResultRequest {
	format: Option<JobResultResponseFormat>,
	limit: Option<u64>,
	offset: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct JobResultJsonResponse {
	results: Vec<serde_json::Value>,
}

/// NOTE: Type conversions from postgres to rust types
/// are according to the table given by
/// [sqlx here](https://docs.rs/sqlx/latest/sqlx/postgres/types/index.html)
#[derive(Debug, Serialize, PartialEq, Eq)]
pub enum ValidStatus {
	Running,
	Completed,
}

/// Job record stores the information about a submitted job
///
/// `job_status` field is an update on read field. It's
/// status will be derived from counting number of
/// completed email verification tasks. It will be updated
/// with the most recent status of the job.
#[derive(sqlx::FromRow, Debug, Serialize)]
pub struct JobRecord {
	id: i32,
	created_at: DateTime<Utc>,
	total_records: i32,
}

/// Summary of a bulk verification job status
#[derive(Debug, Serialize)]
pub struct JobStatusSummaryResponseBody {
	total_safe: i32,
	total_risky: i32,
	total_invalid: i32,
	total_unknown: i32,
}

/// Complete information about a bulk verification job
#[derive(Debug, Serialize)]
pub struct JobStatusResponseBody {
	job_id: i32,
	created_at: DateTime<Utc>,
	total_records: i32,
	total_processed: i32,
	summary: JobStatusSummaryResponseBody,
	job_status: ValidStatus,
}
/// Wrapper for serde json value to convert
/// into a csv response
#[derive(Debug)]
struct CsvWrapper(serde_json::Value);

/// Simplified output of `CheckEmailOutput` struct
/// for csv fields
#[derive(Debug, Serialize)]
struct JobResultCsvResponse {
	input: String,
	is_reachable: String,
	#[serde(rename = "misc.is_disposable")]
	misc_is_disposable: bool,
	#[serde(rename = "misc.is_role_account")]
	misc_is_role_account: bool,
	#[serde(rename = "mx.accepts_mail")]
	mx_accepts_mail: bool,
	#[serde(rename = "smtp.can_connect")]
	smtp_can_connect: bool,
	#[serde(rename = "smtp.has_full_inbox")]
	smtp_has_full_inbox: bool,
	#[serde(rename = "smtp.is_catch_all")]
	smtp_is_catch_all: bool,
	#[serde(rename = "smtp.is_deliverable")]
	smtp_is_deliverable: bool,
	#[serde(rename = "smtp.is_disabled")]
	smtp_is_disabled: bool,
	#[serde(rename = "syntax.is_valid_syntax")]
	syntax_is_valid_syntax: bool,
	#[serde(rename = "syntax.domain")]
	syntax_domain: String,
	#[serde(rename = "syntax.username")]
	syntax_username: String,
	error: Option<String>,
}

/// Convert csv wrapper to csv response
/// Performs multiple allocations for string fields
/// throw error if field is missing
impl TryFrom<CsvWrapper> for JobResultCsvResponse {
	type Error = &'static str;

	fn try_from(value: CsvWrapper) -> Result<Self, Self::Error> {
		let mut input: String = String::default();
		let mut is_reachable: String = String::default();
		let mut misc_is_disposable: bool = false;
		let mut misc_is_role_account: bool = false;
		let mut mx_accepts_mail: bool = false;
		let mut smtp_can_connect: bool = false;
		let mut smtp_has_full_inbox: bool = false;
		let mut smtp_is_catch_all: bool = false;
		let mut smtp_is_deliverable: bool = false;
		let mut smtp_is_disabled: bool = false;
		let mut syntax_is_valid_syntax: bool = false;
		let mut syntax_domain: String = String::default();
		let mut syntax_username: String = String::default();
		let mut error: Option<String> = None;

		let top_level = value
			.0
			.as_object()
			.ok_or("Failed to find top level object")?;
		for (key, val) in top_level.keys().zip(top_level.values()) {
			match key.as_str() {
				"input" => input = val.as_str().ok_or("input should be a string")?.to_string(),
				"is_reachable" => {
					is_reachable = val
						.as_str()
						.ok_or("is_reachable should be a string")?
						.to_string()
				}
				"misc" => {
					let misc_obj = val.as_object().ok_or("misc field should be an object")?;
					for (key, val) in misc_obj.keys().zip(misc_obj.values()) {
						match key.as_str() {
							"error" => error = Some(val.to_string()),
							"is_disposable" => {
								misc_is_disposable =
									val.as_bool().ok_or("is_disposable should be a boolean")?
							}
							"is_role_account" => {
								misc_is_role_account =
									val.as_bool().ok_or("is_role_account should be a boolean")?
							}
							_ => {}
						}
					}
				}
				"mx" => {
					let mx_obj = val.as_object().ok_or("mx field should be an object")?;
					for (key, val) in mx_obj.keys().zip(mx_obj.values()) {
						match key.as_str() {
							"error" => error = Some(val.to_string()),
							"accepts_email" => {
								mx_accepts_mail =
									val.as_bool().ok_or("accepts_email should be a boolean")?
							}
							_ => {}
						}
					}
				}
				"smtp" => {
					let smtp_obj = val.as_object().ok_or("mx field should be an object")?;
					for (key, val) in smtp_obj.keys().zip(smtp_obj.values()) {
						match key.as_str() {
							"error" => error = Some(val.to_string()),
							"can_connect_smtp" => {
								smtp_can_connect = val
									.as_bool()
									.ok_or("can_connect_smtp should be a boolean")?
							}
							"has_full_inbox" => {
								smtp_has_full_inbox =
									val.as_bool().ok_or("has_full_inbox should be a boolean")?
							}
							"is_catch_all" => {
								smtp_is_catch_all =
									val.as_bool().ok_or("is_catch_all should be a boolean")?
							}
							"is_deliverable" => {
								smtp_is_deliverable =
									val.as_bool().ok_or("is_deliverable should be a boolean")?
							}
							"is_disabled" => {
								smtp_is_disabled =
									val.as_bool().ok_or("is_disabled should be a boolean")?
							}
							_ => {}
						}
					}
				}
				"syntax" => {
					let syntax_obj = val.as_object().ok_or("syntax field should be an object")?;
					for (key, val) in syntax_obj.keys().zip(syntax_obj.values()) {
						match key.as_str() {
							"error" => error = Some(val.to_string()),
							"is_valid_syntax" => {
								syntax_is_valid_syntax =
									val.as_bool().ok_or("is_valid_syntax should be a boolean")?
							}
							"username" => {
								syntax_username = val
									.as_str()
									.ok_or("username should be a string")?
									.to_string()
							}
							"domain" => {
								syntax_domain =
									val.as_str().ok_or("domain should be a string")?.to_string()
							}
							_ => {}
						}
					}
				}
				// ignore unknown fields
				_ => {}
			}
		}

		Ok(JobResultCsvResponse {
			input,
			is_reachable,
			misc_is_disposable,
			misc_is_role_account,
			mx_accepts_mail,
			smtp_can_connect,
			smtp_has_full_inbox,
			smtp_is_catch_all,
			smtp_is_deliverable,
			smtp_is_disabled,
			syntax_domain,
			syntax_is_valid_syntax,
			syntax_username,
			error,
		})
	}
}

async fn job_result(
	job_id: i32,
	req: JobResultRequest,
	conn_pool: Pool<Postgres>,
	// ) -> Either<Result<impl warp::Reply, warp::Rejection>, Result<impl warp::Reply, warp::Rejection>> {
) -> Result<impl warp::Reply, warp::Rejection> {
	let format = req.format.unwrap_or(JobResultResponseFormat::Json);
	match format {
		JobResultResponseFormat::Json => {
			let data = job_result_json(
				job_id,
				req.limit.unwrap_or(50),
				req.offset.unwrap_or(0),
				conn_pool,
			)
			.await?;

			let reply =
				serde_json::to_vec(&JobResultJsonResponse { results: data }).map_err(|e| {
					log::error!(
						target:"reacher",
						"Failed to convert json results to string for [job_id={}] with [error={}]",
						job_id,
						e
					);

					ReacherError::Json()
				})?;

			Ok(warp::reply::with_header(
				reply,
				"Content-Type",
				"application/json",
			))
		}
		JobResultResponseFormat::Csv => {
			let data = job_result_csv(
				job_id,
				req.limit.unwrap_or(5000),
				req.offset.unwrap_or(0),
				conn_pool,
			)
			.await?;

			Ok(warp::reply::with_header(data, "Content-Type", "text/csv"))
		}
	}
}

async fn job_result_csv(
	job_id: i32,
	limit: u64,
	offset: u64,
	conn_pool: Pool<Postgres>,
) -> Result<Vec<u8>, warp::Rejection> {
	let query = sqlx::query!(
		r#"
		SELECT result FROM email_results
		WHERE job_id = $1
		ORDER BY id
		LIMIT $2 OFFSET $3
		"#,
		job_id,
		limit as i64,
		offset as i64
	);

	let mut wtr = WriterBuilder::new().has_headers(true).from_writer(vec![]);

	for json_value in conn_pool
		.fetch_all(query)
		.await
		.map_err(|e| {
			log::error!(
				target:"reacher",
				"Failed to get results for [job_id={}] with [error={}]",
				job_id,
				e
			);

			ReacherError::from(e)
		})?
		.iter()
		.map(|row| row.get("result"))
	{
		let result_csv: JobResultCsvResponse = CsvWrapper(json_value).try_into().map_err(|e| {
			log::error!(
				target:"reacher",
				"Failed to convert json to csv output struct for [job_id={}] [limit={}] [offset={}] to csv with [error={}]",
				job_id,
				limit,
				offset,
				e
			);

			ReacherError::Csv()
		})?;
		wtr.serialize(result_csv).map_err(|e| {
			log::error!(
				target:"reacher",
				"Failed to serialize result for [job_id={}] [limit={}] [offset={}] to csv with [error={}]",
				job_id,
				limit,
				offset,
				e
			);

			ReacherError::Csv()
		})?;
	}

	let data = wtr.into_inner().map_err(|e| {
		log::error!(
			target:"reacher",
			"Failed to convert results for [job_id={}] [limit={}] [offset={}] to csv with [error={}]",
			job_id,
			limit,
			offset,
			e
		);

		ReacherError::Csv()
	})?;

	Ok(data)
}

async fn job_result_json(
	job_id: i32,
	limit: u64,
	offset: u64,
	conn_pool: Pool<Postgres>,
) -> Result<Vec<serde_json::Value>, warp::Rejection> {
	let query = sqlx::query!(
		r#"
		SELECT result FROM email_results
		WHERE job_id = $1
		ORDER BY id
		LIMIT $2 OFFSET $3
		"#,
		job_id,
		limit as i64,
		offset as i64
	);

	let rows: Vec<serde_json::Value> = conn_pool
		.fetch_all(query)
		.await
		.map_err(|e| {
			log::error!(
				target:"reacher",
				"Failed to get results for [job_id={}] [limit={}] [offset={}] with [error={}]",
				job_id,
				limit,
				offset,
				e
			);

			ReacherError::from(e)
		})?
		.iter()
		.map(|row| row.get("result"))
		.collect();

	Ok(rows)
}

async fn job_status(
	job_id: i32,
	conn_pool: Pool<Postgres>,
) -> Result<impl warp::Reply, warp::Rejection> {
	let job_rec = sqlx::query_as!(
		JobRecord,
		r#"
		SELECT id, created_at, total_records FROM bulk_jobs
		WHERE id = $1
		LIMIT 1
		"#,
		job_id
	)
	.fetch_one(&conn_pool)
	.await
	.map_err(|e| {
		log::error!(
			target:"reacher",
			"Failed to get job record for [job_id={}] with [error={}]",
			job_id,
			e
		);
		ReacherError::from(e)
	})?;

	let agg_info = sqlx::query!(
		r#"
		SELECT
			COUNT(*) as total_processed,
			COUNT(CASE WHEN result ->> 'is_reachable' LIKE 'safe' THEN 1 END) as safe_count,
			COUNT(CASE WHEN result ->> 'is_reachable' LIKE 'risky' THEN 1 END) as risky_count,
			COUNT(CASE WHEN result ->> 'is_reachable' LIKE 'invalid' THEN 1 END) as invalid_count,
			COUNT(CASE WHEN result ->> 'is_reachable' LIKE 'unknown' THEN 1 END) as unknown_count
		FROM email_results
		WHERE job_id = $1
		"#,
		job_id
	)
	.fetch_one(&conn_pool)
	.await
	.map_err(|e| {
		log::error!(
			target:"reacher/v0/bulk/",
			"Failed to get aggregate info for [job_id={}] with [error={}]",
			job_id,
			e
		);
		ReacherError::from(e)
	})?;

	let job_status = if (agg_info.total_processed.unwrap() as i32) < job_rec.total_records {
		ValidStatus::Running
	} else {
		ValidStatus::Completed
	};

	Ok(warp::reply::json(&JobStatusResponseBody {
		job_id: job_rec.id,
		created_at: job_rec.created_at,
		total_records: job_rec.total_records,
		total_processed: agg_info.total_processed.unwrap() as i32,
		summary: JobStatusSummaryResponseBody {
			total_safe: agg_info.safe_count.unwrap() as i32,
			total_risky: agg_info.risky_count.unwrap() as i32,
			total_invalid: agg_info.invalid_count.unwrap() as i32,
			total_unknown: agg_info.unknown_count.unwrap() as i32,
		},
		job_status,
	}))
}

pub fn get_job_status(
	conn_pool: Pool<Postgres>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
	warp::path!("v0" / "bulk" / i32)
		.and(warp::get())
		.and_then(move |job_id| job_status(job_id, conn_pool.clone()))
		// View access logs by setting `RUST_LOG=reacher`.
		.with(warp::log("reacher"))
}

pub fn get_job_result(
	conn_pool: Pool<Postgres>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
	warp::path!("v0" / "bulk" / i32 / "download")
		.and(warp::get())
		.and(warp::query::<JobResultRequest>())
		.and_then(move |job_id, req| job_result(job_id, req, conn_pool.clone()))
		// View access logs by setting `RUST_LOG=reacher`.
		.with(warp::log("reacher"))
}
