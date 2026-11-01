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
    #[error("{tool}을(를) 찾지 못했습니다. [설정] 탭에서 설치 상태를 확인하세요. {hint}")]
    ToolMissing { tool: String, hint: String },
    #[error("{tool} 실행 실패: {message}")]
    Tool { tool: String, message: String },
    #[error("시간 초과({0}초)로 작업을 멈췄습니다")]
    Timeout(u64),
    #[error("취소했습니다")]
    Cancelled,
    #[error("이미지를 처리할 수 없습니다 ({path}): {message}")]
    Image { path: PathBuf, message: String },
    #[error("이 파일 형식은 이 작업에 쓸 수 없습니다: {0}")]
    Unsupported(String),
    /// 0.3.0 PDF 편집: 암호를 넣어 다시 시도하면 되는 경우(화면이 암호 입력 칸을 띄움)
    #[error("암호가 걸린 PDF입니다. 암호를 넣어 주세요 ({})", .0.display())]
    PasswordRequired(PathBuf),
    #[error("암호가 맞지 않습니다. 다시 넣어 주세요 ({})", .0.display())]
    WrongPassword(PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn io_err(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.into();
    move |source| Error::Io { path, source }
}
