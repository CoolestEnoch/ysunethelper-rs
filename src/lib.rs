pub mod cas;
pub mod config;
pub mod daemon;
pub mod http;
pub mod portal;
pub mod probe;
pub mod shutdown;

#[derive(Debug, thiserror::Error)]
pub enum Failure {
    #[error("用户名或密码错误: {0}")]
    Credentials(String),
    #[error("需要验证码或二次认证: {0}")]
    Challenge(String),
    #[error("认证网关暂时封禁了当前 IP")]
    Blocked,
    #[error("CAS 会话失效，请重新登录")]
    Unauthenticated,
    #[error("CAS 账密登录步骤失败: {0}")]
    Login(#[source] anyhow::Error),
    #[error("配置或凭据错误，需要人工处理: {0}")]
    Hard(String),
    #[error("网络传输失败: {0}")]
    Network(#[source] anyhow::Error),
}
pub fn is_network(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|e| matches!(e.downcast_ref::<Failure>(), Some(Failure::Network(_))))
}
