// 应用错误类型。实现 Serialize 以便跨 IPC 边界返回前端(序列化为字符串)。

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("数据解析错误: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("TRAE 桌面凭证错误: {0}")]
    Credential(String),
    #[error("Windows DPAPI 错误: {0}")]
    Dpapi(String),
    #[error("未找到账号: {0}")]
    NotFound(String),
    #[error("启动失败: {0}")]
    Launch(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_ref())
    }
}

pub type AppResult<T> = Result<T, AppError>;
