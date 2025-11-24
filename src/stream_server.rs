// src/stream_server.rs

use actix_files::NamedFile;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// Type alias for the shared state, holding the optional path of the video to be streamed.
pub type StreamState = Arc<Mutex<Option<PathBuf>>>;

/// HTTP handler for the `/stream` endpoint.
/// Serves the video file specified in the shared `StreamState`.
async fn stream_video(
    req: HttpRequest,
    state: web::Data<StreamState>,
) -> Result<HttpResponse, actix_web::Error> {
    let path_to_serve = match state.get_ref().lock().unwrap().clone() {
        Some(p) => p,
        None => return Ok(HttpResponse::NotFound().body("No video selected for streaming")),
    };

    // NamedFile automatically handles Range requests for video seeking.
    let named_file = NamedFile::open_async(path_to_serve).await?;
    Ok(named_file.into_response(&req))
}

/// Configures and starts the Actix web server for video streaming.
///
/// # Arguments
///
/// * `host` - The host address to bind the server to.
/// * `port` - The port number to bind the server to.
/// * `app_state` - The shared application state (`StreamState`) containing the video path.
///
/// # Returns
///
/// A `std::io::Result` containing the Actix server instance if binding is successful.
pub fn run_server(
    host: String,
    port: u16,
    app_state: web::Data<StreamState>, // Must be Send + Sync.
) -> std::io::Result<actix_web::dev::Server> {
    let server = HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone()) // Share state with HTTP handlers.
            .route("/stream", web::get().to(stream_video))
    })
    .workers(1) // Typically, 1 worker is sufficient for this kind of local streaming.
    .bind((host, port))?
    .run();

    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[actix_web::test]
    async fn test_stream_video_no_selection() {
        let state: StreamState = Arc::new(Mutex::new(None));
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state.clone()))
                .route("/stream", web::get().to(stream_video))
        ).await;

        let req = test::TestRequest::get().uri("/stream").to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);
    }

    #[actix_web::test]
    async fn test_stream_video_with_selection() {
        // Create a dummy video file
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "dummy video content").unwrap();
        let temp_path = temp_file.path().to_path_buf();

        let state: StreamState = Arc::new(Mutex::new(Some(temp_path)));
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state.clone()))
                .route("/stream", web::get().to(stream_video))
        ).await;

        let req = test::TestRequest::get().uri("/stream").to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
    }
}
