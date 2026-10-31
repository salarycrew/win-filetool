use std::path::PathBuf;

/// 사용자에게 그대로 보여 줄 수 있도록 메시지는 한국어로 둔다.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("파일 입출력 오류 ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("zip 파일을 읽거나 쓸 수 없습니다 ({path}): {message}")]
    Zip { path: PathBuf, message: String },
    #[error("PDF를 처리할 수 없습니다 ({path}): {message}")]
    Pdf { path: PathBuf, message: String },
    #[error("페이지 지정이 올바르지 않습니다: {0}")]
    PageSpec(String),
    #[error("이미 같은 이름이 있어 덮어쓰지 않았습니다: {0}")]
    AlreadyExists(PathBuf),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn io_err(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.into();
    move |source| Error::Io { path, source }
}
