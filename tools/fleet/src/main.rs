use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

const RPC: &str = "https://rpc.testnet.near.org";

struct Target {
    var: &'static str,
    artifact: &'static str,
    tag_name: &'static str,
    probe: &'static str,
}

impl Target {
    fn account(&self) -> String {
        account(self.var)
    }
}

fn account(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| panic!("set {var} to the account this run should act on"))
}

const FLEET: [Target; 5] = [
    Target {
        var: "DEPLOYER_ACCOUNT",
        artifact: "wallet_impl_deployer",
        tag_name: "wallet-impl-deployer",
        probe: "config",
    },
    Target {
        var: "ROOT_ACCOUNT",
        artifact: "registrar",
        tag_name: "registrar",
        probe: "config",
    },
    Target {
        var: "EXTENSION_ACCOUNT",
        artifact: "hos_extension",
        tag_name: "hos-extension",
        probe: "get_admins",
    },
    Target {
        var: "RECOVERY_ACCOUNT",
        artifact: "mpc_recovery",
        tag_name: "mpc-recovery",
        probe: "owner",
    },
    Target {
        var: "REGISTRY_ACCOUNT",
        artifact: "tla_registry",
        tag_name: "tla-registry",
        probe: "get_stats",
    },
];

fn repo_root() -> Result<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if !out.status.success() {
        bail!("not inside a git repository");
    }
    Ok(PathBuf::from(String::from_utf8(out.stdout)?.trim()))
}

fn tag_commit(tag: &str) -> Result<String> {
    let out = Command::new("git")
        .args(["rev-list", "-n1", tag])
        .output()?;
    if !out.status.success() {
        bail!("unknown tag {tag}");
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

fn tag_hashes(tag: &str) -> Result<Vec<(String, String)>> {
    let out = Command::new("git").args(["tag", "-n99", tag]).output()?;
    let body = String::from_utf8(out.stdout)?;
    let mut found = Vec::new();
    for line in body.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(hash), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if FLEET.iter().any(|t| t.tag_name == name) || name == "hos-wallet" {
            found.push((name.to_string(), hash.to_string()));
        }
    }
    if found.is_empty() {
        bail!("tag {tag} carries no hash table");
    }
    Ok(found)
}

fn bs58_of(bytes: &[u8]) -> String {
    bs58::encode(Sha256::digest(bytes)).into_string()
}

fn embedded_commit(bytes: &[u8]) -> Option<String> {
    let needle = b"?rev=";
    bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| {
            bytes[i + needle.len()..]
                .iter()
                .take(40)
                .map(|b| *b as char)
                .collect()
        })
}

fn rpc(body: serde_json::Value) -> Result<serde_json::Value> {
    let resp: serde_json::Value = ureq::post(RPC)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())?
        .into_json()?;
    Ok(resp)
}

fn chain_hash(account: &str) -> Result<String> {
    let v = rpc(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "query",
        "params": {"request_type": "view_account", "finality": "final", "account_id": account}
    }))?;
    v["result"]["code_hash"]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("no code_hash for {account}"))
}

fn probe(account: &str, method: &str) -> Result<()> {
    let v = rpc(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "query",
        "params": {"request_type": "call_function", "finality": "final",
                   "account_id": account, "method_name": method, "args_base64": "e30="}
    }))?;
    if let Some(err) = v["result"]["error"].as_str() {
        bail!("{account}.{method} failed: {err}");
    }
    if v["result"]["result"].is_null() {
        bail!("{account}.{method} returned nothing: {v}");
    }
    Ok(())
}

fn staged_dir(root: &Path, tag: &str) -> PathBuf {
    root.join("artifacts").join(tag)
}

fn stage(tag: &str) -> Result<()> {
    let root = repo_root()?;
    let commit = tag_commit(tag)?;
    let table = tag_hashes(tag)?;
    let dest = staged_dir(&root, tag);
    std::fs::create_dir_all(&dest)?;

    for (name, want) in &table {
        let artifact = name.replace('-', "_");
        let src = root
            .join("target/near")
            .join(&artifact)
            .join(format!("{artifact}.wasm"));
        let bytes = std::fs::read(&src).with_context(|| format!("read {}", src.display()))?;

        let got = bs58_of(&bytes);
        if &got != want {
            bail!("{name}: built artifact is {got}, tag {tag} says {want}");
        }
        match embedded_commit(&bytes) {
            Some(c) if c == commit => {}
            Some(c) => bail!("{name}: artifact was built at {c}, tag {tag} is {commit}"),
            None => bail!("{name}: artifact carries no NEP-330 commit, not a reproducible build"),
        }
        std::fs::write(dest.join(format!("{artifact}.wasm")), &bytes)?;
        println!("staged {name:<22} {got}");
    }
    println!("\n{} artifact(s) staged at {}", table.len(), dest.display());
    Ok(())
}

fn load_staged(root: &Path, tag: &str, artifact: &str) -> Result<Vec<u8>> {
    let path = staged_dir(root, tag).join(format!("{artifact}.wasm"));
    std::fs::read(&path)
        .with_context(|| format!("{} missing; run `fleet stage {tag}` first", path.display()))
}

fn verify(tag: &str) -> Result<bool> {
    let root = repo_root()?;
    let commit = tag_commit(tag)?;
    let table = tag_hashes(tag)?;
    let mut clean = true;

    println!("tag {tag} -> {commit}\n");
    println!("{:<26} {:<46} state", "account", "on chain");
    for t in &FLEET {
        let want = table
            .iter()
            .find(|(n, _)| n == t.tag_name)
            .map(|(_, h)| h.clone())
            .with_context(|| format!("tag has no line for {}", t.tag_name))?;
        let bytes = load_staged(&root, tag, t.artifact)?;
        let staged = bs58_of(&bytes);
        if staged != want {
            bail!("{}: staged artifact drifted from the tag", t.account());
        }
        let on_chain = chain_hash(&t.account())?;
        let state = if on_chain == want {
            match probe(&t.account(), t.probe) {
                Ok(()) => "current",
                Err(_) => {
                    clean = false;
                    "CURRENT BUT UNREADABLE"
                }
            }
        } else {
            clean = false;
            "stale"
        };
        println!("{:<26} {:<46} {}", t.account(), on_chain, state);
    }
    Ok(clean)
}

fn deploy_one(t: &Target, path: &Path, want: &str) -> Result<()> {
    let previous = chain_hash(&t.account())?;
    if previous == want {
        println!("{:<26} already current, skipping", t.account());
        return Ok(());
    }
    println!("{:<26} {previous} -> {want}", t.account());

    let status = Command::new("near")
        .args([
            "contract",
            "deploy",
            &t.account(),
            "use-file",
            path.to_str().context("artifact path is not utf8")?,
            "without-init-call",
            "network-config",
            "testnet",
            "sign-with-keychain",
            "send",
        ])
        .status()?;
    if !status.success() {
        bail!("{}: near deploy exited {status}", t.account());
    }

    let now = chain_hash(&t.account())?;
    if now != want {
        bail!(
            "{}: deployed but chain reports {now}, expected {want}",
            t.account()
        );
    }
    if let Err(e) = probe(&t.account(), t.probe) {
        bail!(
            "{}: DEPLOYED BUT UNREADABLE ({e}). Roll back now with:\n  \
             near contract deploy {} use-file <artifact with hash {previous}> \
             without-init-call network-config testnet sign-with-keychain send",
            t.account(),
            t.account()
        );
    }
    println!("{:<26} deployed and readable", t.account());
    Ok(())
}

fn deployer() -> String {
    account("DEPLOYER_ACCOUNT")
}

fn full_access_keys(account: &str) -> Result<usize> {
    let v = rpc(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "query",
        "params": {"request_type": "view_access_key_list", "finality": "final",
                   "account_id": account}
    }))?;
    let keys = v["result"]["keys"]
        .as_array()
        .with_context(|| format!("no key list for {account}"))?;
    Ok(keys
        .iter()
        .filter(|k| k["access_key"]["permission"] == "FullAccess")
        .count())
}

fn the_publish_path_is_the_only_path() -> Result<()> {
    let deployer = deployer();
    let held = full_access_keys(&deployer)?;
    if held > 0 {
        bail!(
            "{deployer} holds {held} FullAccess key(s). Any one of them can publish wallet \
             code directly with a DeployGlobalContract action, skipping gd_approve and the \
             approval delay entirely, and every leased account follows the new code at once. \
             Until that account holds none, the delay is a convention rather than a control."
        );
    }
    println!("{deployer:<26} holds no FullAccess key, publishing is contract-gated");
    Ok(())
}

fn deploy(tag: &str) -> Result<()> {
    let root = repo_root()?;
    let table = tag_hashes(tag)?;
    the_publish_path_is_the_only_path()?;
    verify(tag).ok();
    println!();

    for t in &FLEET {
        let want = table
            .iter()
            .find(|(n, _)| n == t.tag_name)
            .map(|(_, h)| h.clone())
            .with_context(|| format!("tag has no line for {}", t.tag_name))?;
        let path = staged_dir(&root, tag).join(format!("{}.wasm", t.artifact));
        let bytes = load_staged(&root, tag, t.artifact)?;
        if bs58_of(&bytes) != want {
            bail!("{}: staged artifact does not match the tag", t.account());
        }
        deploy_one(t, &path, &want)?;
    }

    println!();
    if verify(tag)? {
        println!("\nfleet matches {tag} and every contract answers");
    } else {
        bail!("fleet does not match {tag} after deploying");
    }
    Ok(())
}

fn up(tag: &str) -> Result<()> {
    println!("== stage");
    stage(tag)?;
    println!("\n== keys");
    the_publish_path_is_the_only_path()?;
    println!("\n== deploy");
    deploy(tag)?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(cmd), Some(tag)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: fleet <up|stage|verify|keys|deploy> <tag>");
        std::process::exit(2);
    };
    match cmd.as_str() {
        "up" => up(tag),
        "stage" => stage(tag),
        "keys" => the_publish_path_is_the_only_path(),
        "verify" => {
            if !verify(tag)? {
                std::process::exit(1);
            }
            Ok(())
        }
        "deploy" => deploy(tag),
        other => {
            eprintln!("unknown command {other}");
            std::process::exit(2);
        }
    }
}
