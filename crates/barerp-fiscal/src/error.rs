//! Erros da emissão fiscal. As mensagens são as mesmas que o servidor Node devolve hoje à tela.

#[derive(Debug, thiserror::Error)]
pub enum FiscalError {
    /// Dado de entrada inválido (o servidor responde 400 antes de chamar a Focus).
    #[error("{0}")]
    Validacao(String),
    /// A Focus NFe recusou a requisição (HTTP fora de 2xx).
    #[error("{mensagem}")]
    Focus { http_status: u16, mensagem: String, detalhes: serde_json::Value },
    /// A requisição foi aceita, mas a SEFAZ não autorizou (erro_autorizacao, denegado, cancelado...).
    /// `status` é o status da Focus, para quem chama gravar a nota com ele.
    #[error("{mensagem}")]
    Sefaz { status: String, mensagem: String, detalhes: serde_json::Value },
    /// Falha de rede ou resposta ilegível.
    #[error("Erro ao conectar com a Focus NFe: {0}")]
    Rede(#[from] reqwest::Error),
}

impl FiscalError {
    pub(crate) fn validacao(msg: impl Into<String>) -> Self {
        Self::Validacao(msg.into())
    }
}
