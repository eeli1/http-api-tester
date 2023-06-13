mod test_request;

use test_request::TestRequest;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() -> Result<()> {
    for req in TestRequest::parse_http_file("http-api/test.http".to_string())? {
        req.test().await?;
    }

    Ok(())
}
