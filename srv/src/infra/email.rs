use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::header::ContentType,
    transport::smtp::authentication::{Credentials, Mechanism},
};
use tracing::{error, info};

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
};

/// SMTP 邮件客户端。
///
/// 邮件发送属于基础设施能力，业务 handler 只传入收件人和验证码，不直接拼 SMTP
/// transport。这样后续切换邮件服务商时，只需要调整这个模块。
#[derive(Clone)]
pub struct EmailClient {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: String,
}

impl EmailClient {
    pub fn new(config: &AppConfig) -> AppResult<Self> {
        let credentials =
            Credentials::new(config.smtp_username.clone(), config.smtp_password.clone());
        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)
            .map_err(|source| AppError::Email {
                message: format!("SMTP relay 初始化失败: {source}"),
            })?
            .port(config.smtp_port)
            .credentials(credentials)
            .authentication(vec![Mechanism::Plain])
            .build();

        info!(
            smtp_host = %config.smtp_host,
            smtp_port = config.smtp_port,
            smtp_from = %config.smtp_from,
            "SMTP 邮件客户端初始化完成"
        );

        Ok(Self {
            transport,
            from: config.smtp_from.clone(),
        })
    }

    /// 发送注册验证码。
    ///
    /// 邮件正文保持简单文本，避免 HTML 邮件在不同客户端上的兼容差异影响注册流程。
    pub async fn send_register_code(&self, receiver: &str, code: &str) -> AppResult<()> {
        let email = Message::builder()
            .from(self.from.parse().map_err(|source| AppError::Email {
                message: format!("发件人地址无效: {source}"),
            })?)
            .to(receiver.parse().map_err(|source| AppError::Email {
                message: format!("收件人地址无效: {source}"),
            })?)
            .subject("aestus Gateway 注册验证码")
            .header(ContentType::TEXT_PLAIN)
            .body(format!(
                "您的注册验证码是：{code}\n验证码有效期较短，请尽快完成注册。"
            ))
            .map_err(|source| AppError::Email {
                message: format!("注册验证码邮件构造失败: {source}"),
            })?;

        self.transport.send(email).await.map_err(|source| {
            error!(receiver, error = %source, "注册验证码邮件发送失败");
            AppError::Email {
                message: source.to_string(),
            }
        })?;

        info!(receiver, "注册验证码邮件发送成功");
        Ok(())
    }
}
