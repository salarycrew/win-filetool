//! 0.3.0 W6 한글 Tauri 명령. 계약은 docs/design.md "## 0.3.0 한글(W6)".
//!
//! 변환 명령은 이름을 바꾸지 않았다: `convert_files`(문서 → PDF/DOCX), `doc_to_pptx`, `extract_images`가
//! 한글이면 코어([`filetool_core::hwp`])에서 알맞은 길(설치된 한글 / LibreOffice / 안내 오류)로 보낸다.
//! 화면은 이 명령으로 되는 기능만 보여 준다.

use std::path::Path;

use filetool_core::hwp::{self, HwpCaps};
use filetool_core::tools::Tool;
use tauri::State;

use crate::{blocking, AppState, CmdResult};

/// 한글 파일 하나의 형식과 할 수 있는 것
/// `{format:"hwp97"|"hwp5"|"hwpx"|"damaged", version?, encrypted, distribution, can_pdf, can_pptx, can_images, route:"hancom"|"libreoffice"|"none", message?}`
#[tauri::command]
pub async fn hwp_capabilities(state: State<'_, AppState>, path: String) -> CmdResult<HwpCaps> {
    let soffice = state.toolset().get(Tool::Soffice).is_ok();
    blocking(move || hwp::capabilities(Path::new(&path), soffice)).await
}
