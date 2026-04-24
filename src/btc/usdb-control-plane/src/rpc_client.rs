use crate::config::{BitcoinAuthMode, ControlPlaneConfig};
use crate::models::{
    BalanceHistoryReadiness, BitcoinBlockHeader, BitcoinBlockchainInfo, EthBlockHeader,
    UsdbIndexerReadiness,
};
use bitcoincore_rpc::bitcoin::PrivateKey;
use bitcoincore_rpc::bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoincore_rpc::bitcoin::key::Secp256k1;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::str::FromStr;

#[derive(Debug, serde::Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

#[derive(Debug, serde::Deserialize)]
struct BitcoinWalletAddressInfo {
    #[serde(default)]
    hdkeypath: Option<String>,
    #[serde(default)]
    ischange: bool,
}

#[derive(Debug, serde::Deserialize)]
struct BitcoinWalletDescriptorsResponse {
    descriptors: Vec<BitcoinWalletDescriptor>,
}

#[derive(Debug, serde::Deserialize)]
struct BitcoinWalletDescriptor {
    desc: String,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    internal: bool,
}

#[derive(Debug)]
struct ParsedPrivateDescriptor {
    kind: String,
    xpriv: Xpriv,
    path_suffix: Vec<String>,
}

/// Shared RPC/HTTP facade used by the control plane to query external services.
///
/// This client intentionally mixes a few transport styles because the control plane
/// has to talk to JSON-RPC services, plain HTTP status endpoints, and the local BTC
/// node wallet RPC during development simulations.
#[derive(Clone)]
pub struct RpcClient {
    client: Client,
}

impl RpcClient {
    /// Builds the shared HTTP client used by all downstream RPC helpers.
    ///
    /// The timeout stays short because these calls back UI-facing status pages and
    /// should fail fast instead of hanging the control plane.
    pub fn new() -> Result<Self, String> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .map_err(|e| {
                let msg = format!("Failed to build HTTP client: {}", e);
                error!("{}", msg);
                msg
            })?;
        Ok(Self { client })
    }

    /// Returns the balance-history network name reported by `get_network_type`.
    pub async fn balance_history_network(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "get_network_type", json!([])).await
    }

    /// Returns the readiness snapshot reported by the balance-history service.
    ///
    /// The control plane uses this to decide whether mint preparation can rely on
    /// BTC ownership history data.
    pub async fn balance_history_readiness(
        &self,
        url: &str,
    ) -> Result<BalanceHistoryReadiness, String> {
        self.json_rpc_call(url, "get_readiness", json!([])).await
    }

    /// Returns the usdb-indexer network name reported by `get_network_type`.
    pub async fn usdb_indexer_network(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "get_network_type", json!([])).await
    }

    /// Returns the readiness snapshot reported by the usdb-indexer service.
    ///
    /// This is used by overview pages and mint-preparation gating to confirm the
    /// indexer is aligned with the active BTC runtime.
    pub async fn usdb_indexer_readiness(&self, url: &str) -> Result<UsdbIndexerReadiness, String> {
        self.json_rpc_call(url, "get_readiness", json!([])).await
    }

    /// Forwards an arbitrary JSON-RPC call to balance-history.
    ///
    /// This exists for control-plane debug and explorer endpoints. It is a generic
    /// pass-through and should not be treated as a stable domain-specific contract.
    pub async fn balance_history_proxy(
        &self,
        url: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.json_rpc_call(url, method, params).await
    }

    /// Forwards an arbitrary JSON-RPC call to usdb-indexer.
    ///
    /// This is primarily used by protocol/debug views where the console needs a thin
    /// proxy rather than a dedicated typed endpoint for every RPC.
    pub async fn usdb_indexer_proxy(
        &self,
        url: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.json_rpc_call(url, method, params).await
    }

    /// Returns the ETHW client version via `web3_clientVersion`.
    pub async fn ethw_client_version(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "web3_clientVersion", json!([]))
            .await
    }

    /// Returns the ETHW chain id via `eth_chainId`.
    pub async fn ethw_chain_id(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "eth_chainId", json!([])).await
    }

    /// Returns the ETHW network id via `net_version`.
    pub async fn ethw_network_id(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "net_version", json!([])).await
    }

    /// Returns the latest ETHW block number as a hex quantity string.
    pub async fn ethw_block_number(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "eth_blockNumber", json!([])).await
    }

    /// Returns the raw ETHW syncing payload from `eth_syncing`.
    ///
    /// Callers can distinguish `false` from an in-progress sync object without losing
    /// fidelity by decoding into `serde_json::Value`.
    pub async fn ethw_syncing(&self, url: &str) -> Result<Value, String> {
        self.json_rpc_call(url, "eth_syncing", json!([])).await
    }

    /// Returns the latest ETHW block header, or `None` if the upstream returned
    /// `result: null`.
    pub async fn ethw_latest_block(&self, url: &str) -> Result<Option<EthBlockHeader>, String> {
        self.json_rpc_call(url, "eth_getBlockByNumber", json!(["latest", false]))
            .await
    }

    /// Performs a lightweight HTTP GET and returns the response status code.
    ///
    /// This is used for health probes where the caller only cares whether the
    /// endpoint is reachable and what status it returned.
    pub async fn http_probe(&self, url: &str) -> Result<u16, String> {
        let response = self.client.get(url).send().await.map_err(|e| {
            let msg = format!("Failed to probe HTTP endpoint {}: {}", url, e);
            warn!("{}", msg);
            msg
        })?;

        Ok(response.status().as_u16())
    }

    /// Fetches the body of a plain HTTP endpoint and requires a success status.
    ///
    /// The control plane uses this for human-readable status pages and artifact-style
    /// endpoints where the raw text body is meaningful.
    pub async fn http_text(&self, url: &str) -> Result<String, String> {
        let response = self.client.get(url).send().await.map_err(|e| {
            let msg = format!("Failed to fetch HTTP endpoint {}: {}", url, e);
            warn!("{}", msg);
            msg
        })?;
        let status = response.status();
        if !status.is_success() {
            let msg = format!(
                "HTTP endpoint {} returned non-success status {}",
                url, status
            );
            warn!("{}", msg);
            return Err(msg);
        }

        response.text().await.map_err(|e| {
            let msg = format!("Failed to read HTTP response body from {}: {}", url, e);
            warn!("{}", msg);
            msg
        })
    }

    /// Returns `getblockchaininfo` from the configured BTC node RPC.
    ///
    /// This is a generic chain probe used by overview/status views for both
    /// development and public runtimes.
    pub async fn bitcoin_blockchain_info(
        &self,
        config: &ControlPlaneConfig,
    ) -> Result<BitcoinBlockchainInfo, String> {
        self.bitcoin_json_rpc_call(config, "getblockchaininfo", json!([]))
            .await
    }

    /// Returns a BTC block header for the supplied block hash.
    ///
    /// The control plane uses this when it needs block-level timing/height detail
    /// after first discovering hashes from other probes.
    pub async fn bitcoin_block_header(
        &self,
        config: &ControlPlaneConfig,
        block_hash: &str,
    ) -> Result<BitcoinBlockHeader, String> {
        self.bitcoin_json_rpc_call(config, "getblockheader", json!([block_hash]))
            .await
    }

    /// Resolves a spendable WIF for a BTC wallet address from the local node wallet.
    ///
    /// This helper is development-only. It currently backs the
    /// `/api/btc/world-sim/dev-signer` flow so the console can auto-sync the browser
    /// dev signer from a selected world-sim identity on regtest/dev-sim.
    ///
    /// It must not be used as a public-runtime wallet export path. On public chains
    /// the console is expected to sign via an external browser wallet instead of
    /// pulling private key material through the control plane.
    pub async fn bitcoin_wallet_resolve_private_key(
        &self,
        config: &ControlPlaneConfig,
        wallet_name: &str,
        address: &str,
    ) -> Result<String, String> {
        self.bitcoin_wallet_ensure_loaded(config, wallet_name)
            .await?;
        let wallet_url = self.bitcoin_wallet_url(config, wallet_name)?;
        let address_info: BitcoinWalletAddressInfo = self
            .bitcoin_json_rpc_call_at_url(
                config,
                wallet_url.as_str(),
                "getaddressinfo",
                json!([address]),
            )
            .await?;
        let hdkeypath = address_info.hdkeypath.as_deref().ok_or_else(|| {
            format!(
                "BTC wallet address {} did not expose an hdkeypath for wallet {}",
                address, wallet_name
            )
        })?;
        let descriptors: BitcoinWalletDescriptorsResponse = self
            .bitcoin_json_rpc_call_at_url(
                config,
                wallet_url.as_str(),
                "listdescriptors",
                json!([true]),
            )
            .await?;
        let active_descriptor = descriptors
            .descriptors
            .iter()
            .find(|descriptor| {
                descriptor.active
                    && descriptor.internal == address_info.ischange
                    && descriptor.desc.contains("prv")
            })
            .ok_or_else(|| {
                format!(
                    "No active private descriptor matched wallet {} (ischange={})",
                    wallet_name, address_info.ischange
                )
            })?;

        derive_private_wif_from_descriptor(&active_descriptor.desc, hdkeypath)
    }

    /// Ensures a local development BTC wallet is loaded before wallet-scoped RPC.
    ///
    /// World-sim wallets can exist on disk after a persistent restart while not yet
    /// being loaded by Bitcoin Core. The dev-signer path is allowed to load those
    /// deterministic regtest wallets on demand; public-runtime signing must still
    /// go through a browser wallet instead of this control-plane helper.
    async fn bitcoin_wallet_ensure_loaded(
        &self,
        config: &ControlPlaneConfig,
        wallet_name: &str,
    ) -> Result<(), String> {
        let load_result: Result<Value, String> = self
            .bitcoin_json_rpc_call(config, "loadwallet", json!([wallet_name]))
            .await;
        match load_result {
            Ok(_) => Ok(()),
            Err(error) if error.to_ascii_lowercase().contains("already loaded") => Ok(()),
            Err(error) => Err(format!(
                "Failed to load BTC wallet {}: {}",
                wallet_name, error
            )),
        }
    }

    async fn json_rpc_call<T: DeserializeOwned>(
        &self,
        url: &str,
        method: &str,
        params: Value,
    ) -> Result<T, String> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                let msg = format!(
                    "Failed to send RPC request to {} (method={}): {}",
                    url, method, e
                );
                warn!("{}", msg);
                msg
            })?;

        let status = response.status();
        let response_body: Value = response.json().await.map_err(|e| {
            let msg = format!(
                "Failed to decode RPC response from {} (method={}, status={}): {}",
                url, method, status, e
            );
            warn!("{}", msg);
            msg
        })?;
        decode_json_rpc_response(
            response_body,
            "RPC",
            method,
            &format!("{} (status={})", url, status),
        )
    }

    async fn bitcoin_json_rpc_call<T: DeserializeOwned>(
        &self,
        config: &ControlPlaneConfig,
        method: &str,
        params: Value,
    ) -> Result<T, String> {
        self.bitcoin_json_rpc_call_at_url(config, &config.bitcoin.url, method, params)
            .await
    }

    fn bitcoin_wallet_url(
        &self,
        config: &ControlPlaneConfig,
        wallet_name: &str,
    ) -> Result<String, String> {
        let mut url = reqwest::Url::parse(&config.bitcoin.url).map_err(|e| {
            let msg = format!("Failed to parse BTC RPC URL {}: {}", config.bitcoin.url, e);
            warn!("{}", msg);
            msg
        })?;
        let mut segments = url.path_segments_mut().map_err(|_| {
            let msg = format!(
                "BTC RPC URL {} cannot be extended with wallet path segments",
                config.bitcoin.url
            );
            warn!("{}", msg);
            msg
        })?;
        segments.pop_if_empty();
        segments.push("wallet");
        segments.push(wallet_name);
        drop(segments);
        Ok(url.to_string())
    }

    async fn bitcoin_json_rpc_call_at_url<T: DeserializeOwned>(
        &self,
        config: &ControlPlaneConfig,
        url: &str,
        method: &str,
        params: Value,
    ) -> Result<T, String> {
        let request = json!({
            "jsonrpc": "1.0",
            "id": "usdb-control-plane",
            "method": method,
            "params": params,
        });

        let mut builder = self.client.post(url).json(&request);
        match config.bitcoin.auth_mode {
            BitcoinAuthMode::None => {}
            BitcoinAuthMode::Userpass => {
                let user =
                    config.bitcoin.rpc_user.as_deref().ok_or_else(|| {
                        "BTC RPC auth_mode=userpass requires rpc_user".to_string()
                    })?;
                let password = config.bitcoin.rpc_password.as_deref().ok_or_else(|| {
                    "BTC RPC auth_mode=userpass requires rpc_password".to_string()
                })?;
                builder = builder.basic_auth(user, Some(password));
            }
            BitcoinAuthMode::Cookie => {
                let cookie_file =
                    config.bitcoin.cookie_file.as_ref().ok_or_else(|| {
                        "BTC RPC auth_mode=cookie requires cookie_file".to_string()
                    })?;
                let cookie_path = config.resolve_runtime_path(cookie_file)?;
                let cookie = std::fs::read_to_string(&cookie_path).map_err(|e| {
                    let msg = format!(
                        "Failed to read BTC cookie file {}: {}",
                        cookie_path.display(),
                        e
                    );
                    warn!("{}", msg);
                    msg
                })?;
                let trimmed = cookie.trim();
                let (user, password) = trimmed.split_once(':').ok_or_else(|| {
                    format!(
                        "Invalid BTC cookie file format at {}",
                        cookie_path.display()
                    )
                })?;
                builder = builder.basic_auth(user, Some(password));
            }
        }

        let response = builder.send().await.map_err(|e| {
            let msg = format!(
                "Failed to send BTC RPC request to {} (method={}): {}",
                url, method, e
            );
            warn!("{}", msg);
            msg
        })?;

        let status = response.status();
        let response_body: Value = response.json().await.map_err(|e| {
            let msg = format!(
                "Failed to decode BTC RPC response from {} (method={}, status={}): {}",
                url, method, status, e
            );
            warn!("{}", msg);
            msg
        })?;
        decode_json_rpc_response(
            response_body,
            "BTC RPC",
            method,
            &format!("{} (status={})", url, status),
        )
    }
}

fn decode_json_rpc_response<T: DeserializeOwned>(
    response_body: Value,
    rpc_label: &str,
    method: &str,
    endpoint: &str,
) -> Result<T, String> {
    let Some(object) = response_body.as_object() else {
        let msg = format!(
            "{} {} returned a non-object response from {}: {}",
            rpc_label, method, endpoint, response_body
        );
        warn!("{}", msg);
        return Err(msg);
    };

    if let Some(error_value) = object.get("error")
        && !error_value.is_null()
    {
        let error: JsonRpcError =
            serde_json::from_value(error_value.clone()).map_err(|decode_error| {
                let msg = format!(
                    "{} {} returned an undecodable error payload from {}: {}",
                    rpc_label, method, endpoint, decode_error
                );
                warn!("{}", msg);
                msg
            })?;
        let msg = if let Some(data) = error.data {
            format!(
                "{} {} returned error {} ({}): {}",
                rpc_label, method, error.code, error.message, data
            )
        } else {
            format!(
                "{} {} returned error {}: {}",
                rpc_label, method, error.code, error.message
            )
        };
        warn!("{}", msg);
        return Err(msg);
    }

    let Some(result_value) = object.get("result") else {
        let msg = format!("{} {} returned neither result nor error", rpc_label, method);
        warn!("{}", msg);
        return Err(msg);
    };

    serde_json::from_value(result_value.clone()).map_err(|error| {
        let msg = format!(
            "Failed to decode {} result from {} (method={}): {}",
            rpc_label, endpoint, method, error
        );
        warn!("{}", msg);
        msg
    })
}

fn derive_private_wif_from_descriptor(desc: &str, hdkeypath: &str) -> Result<String, String> {
    let parsed = parse_private_descriptor(desc)?;
    let address_type = parsed.kind.as_str();
    if address_type != "tr" && address_type != "wpkh" {
        return Err(format!(
            "Unsupported active private descriptor type for dev signer: {}",
            address_type
        ));
    }

    let hdkeypath = DerivationPath::from_str(hdkeypath)
        .map_err(|error| format!("Invalid wallet hdkeypath {}: {}", hdkeypath, error))?;
    // Descriptor wallets do not support `dumpprivkey`, so for dev-sim we reconstruct
    // the concrete child private key from the active private descriptor plus the
    // address-specific hdkeypath exposed by `getaddressinfo`.
    let child_path = resolve_descriptor_child_path(&parsed.path_suffix, &hdkeypath)?;
    let secp = Secp256k1::new();
    let derived = parsed
        .xpriv
        .derive_priv(&secp, &child_path)
        .map_err(|error| {
            format!(
                "Failed to derive wallet private key from descriptor: {}",
                error
            )
        })?;

    Ok(PrivateKey::new(derived.private_key, derived.network).to_wif())
}

fn parse_private_descriptor(desc: &str) -> Result<ParsedPrivateDescriptor, String> {
    let normalized = desc.split('#').next().unwrap_or(desc).trim();
    let open = normalized.find('(').ok_or_else(|| {
        format!(
            "Descriptor {} did not contain an opening '(' for private-key parsing",
            desc
        )
    })?;
    let close = normalized.rfind(')').ok_or_else(|| {
        format!(
            "Descriptor {} did not contain a closing ')' for private-key parsing",
            desc
        )
    })?;
    if close <= open {
        return Err(format!(
            "Descriptor {} had an invalid private-key expression",
            desc
        ));
    }

    let kind = normalized[..open].trim().to_string();
    let inner = normalized[open + 1..close].trim();
    let key_expr = inner
        .rsplit_once(']')
        .map(|(_, rest)| rest)
        .unwrap_or(inner);
    let mut segments = key_expr.split('/').filter(|segment| !segment.is_empty());
    let xpriv_str = segments.next().ok_or_else(|| {
        format!(
            "Descriptor {} did not expose an extended private key expression",
            desc
        )
    })?;
    let xpriv = Xpriv::from_str(xpriv_str)
        .map_err(|error| format!("Failed to parse descriptor xpriv {}: {}", xpriv_str, error))?;

    Ok(ParsedPrivateDescriptor {
        kind,
        xpriv,
        path_suffix: segments.map(str::to_string).collect(),
    })
}

fn resolve_descriptor_child_path(
    path_suffix: &[String],
    hdkeypath: &DerivationPath,
) -> Result<DerivationPath, String> {
    if path_suffix.is_empty() {
        return Ok(Vec::<ChildNumber>::new().into());
    }

    let hd_components = hdkeypath.as_ref();
    if hd_components.len() < path_suffix.len() {
        return Err(format!(
            "Wallet hdkeypath {} is shorter than descriptor suffix /{}",
            hdkeypath,
            path_suffix.join("/")
        ));
    }

    let hd_tail = &hd_components[hd_components.len() - path_suffix.len()..];
    let mut child_path = Vec::with_capacity(path_suffix.len());
    for (segment, actual) in path_suffix.iter().zip(hd_tail.iter()) {
        if segment == "*" {
            if actual.is_hardened() {
                return Err(format!(
                    "Descriptor wildcard cannot match hardened child {} in {}",
                    actual, hdkeypath
                ));
            }
            child_path.push(*actual);
            continue;
        }

        let expected = ChildNumber::from_str(segment).map_err(|error| {
            format!(
                "Descriptor child segment {} is invalid for hdkeypath {}: {}",
                segment, hdkeypath, error
            )
        })?;
        if expected != *actual {
            return Err(format!(
                "Descriptor child segment {} did not match wallet hdkeypath {}",
                segment, hdkeypath
            ));
        }
        child_path.push(expected);
    }

    Ok(child_path.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_private_descriptor_reads_active_taproot_template() {
        let descriptor = "tr([748e048d/86'/1'/0']tprv8gYt1ZpvcHAeECiFs7WkAoL8ATDAvETZAxe4ndKA9yyLicPbau9vPjQQKxetScNbWAjDqScScqdt2sTqfct4ykRhPTXxqWvmnd7h1GjjcXU/0/*)#68csz69e";
        let parsed = parse_private_descriptor(descriptor).unwrap();

        assert_eq!(parsed.kind, "tr");
        assert_eq!(parsed.path_suffix, vec!["0".to_string(), "*".to_string()]);
        assert_eq!(
            parsed.xpriv.to_string(),
            "tprv8gYt1ZpvcHAeECiFs7WkAoL8ATDAvETZAxe4ndKA9yyLicPbau9vPjQQKxetScNbWAjDqScScqdt2sTqfct4ykRhPTXxqWvmnd7h1GjjcXU"
        );
    }

    #[test]
    fn resolve_descriptor_child_path_substitutes_terminal_wildcard() {
        let hdkeypath = DerivationPath::from_str("m/86h/1h/0h/0/0").unwrap();
        let path =
            resolve_descriptor_child_path(&["0".to_string(), "*".to_string()], &hdkeypath).unwrap();

        assert_eq!(path.to_string(), "0/0");
    }

    #[test]
    fn derive_private_wif_from_descriptor_returns_test_wif() {
        let descriptor = "tr([748e048d/86'/1'/0']tprv8gYt1ZpvcHAeECiFs7WkAoL8ATDAvETZAxe4ndKA9yyLicPbau9vPjQQKxetScNbWAjDqScScqdt2sTqfct4ykRhPTXxqWvmnd7h1GjjcXU/0/*)#68csz69e";
        let wif = derive_private_wif_from_descriptor(descriptor, "m/86h/1h/0h/0/0").unwrap();

        assert!(wif.starts_with('c'));
    }

    #[test]
    fn decode_json_rpc_response_accepts_explicit_null_result_for_optional_payloads() {
        let response = json!({
            "jsonrpc": "2.0",
            "result": Value::Null,
            "id": 1
        });

        let decoded =
            decode_json_rpc_response::<Option<Value>>(response, "RPC", "demo", "test").unwrap();

        assert_eq!(decoded, None);
    }
}

/// Decodes an Ethereum-style `0x` hex quantity into `u64`.
///
/// Empty quantities such as `0x` are treated as zero to match the semantics used
/// by upstream JSON-RPC responses.
pub fn decode_hex_quantity(value: &str) -> Result<u64, String> {
    let raw = value.trim_start_matches("0x");
    if raw.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(raw, 16).map_err(|e| format!("Invalid hex quantity {}: {}", value, e))
}
