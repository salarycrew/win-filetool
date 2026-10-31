//! 2a: Everything식 전체 파일 즉시 검색(관리자 권한 없이).
//!
//! - [`index`]: 메모리 색인(이름 창고 + 부모 번호), 저장·읽기(버전 붙은 파일, 바꿔치기 저장)
//! - [`scan`]: 색인을 채우는 원본 [`scan::IndexSource`] — 2a는 폴더 병렬 훑기([`scan::DirWalkSource`]),
//!   2b의 NTFS MFT/USN 원본이 같은 자리에 들어온다
//! - [`ntfs`]·[`mft`]: 2b NTFS 빠른 색인(MFT 직접 읽기, USN 저널 따라잡기·따라가기, 관리자 권한일 때만)
//! - [`live`]: 실시간 변경 알림(Windows `ReadDirectoryChangesW`)
//! - [`query`]: 질의 언어(Everything 일부) 해석·실행·정렬
//! - [`service`]: 불러오기 → 감시 → 배경 스캔 → 저장을 묶은 서비스(앱·CLI 공용)
//! - [`text`]: NFC/NFD·대소문자 무시 키, 한글 초성, 와일드카드

pub mod index;
pub mod live;
pub mod mft;
pub mod ntfs;
pub mod query;
pub mod scan;
pub mod service;
pub mod text;

pub use index::Index;
pub use query::{Query, QueryCtx, Row, SortKey};
pub use service::{default_index_dir, SearchConfig, SearchService, SearchStatus};
