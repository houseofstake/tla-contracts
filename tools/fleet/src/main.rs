use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Network {
    Testnet,
    Mainnet,
}

impl Network {
    fn rpc(self) -> &'static str {
        match self {
            Self::Testnet => "https://rpc.testnet.near.org",
            Self::Mainnet => "https://rpc.mainnet.near.org",
        }
    }

    fn cli_name(self) -> &'static str {
        match self {
            Self::Testnet => "testnet",
            Self::Mainnet => "mainnet",
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.cli_name())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Profile {
    Test,
    Production,
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Test => "test",
            Self::Production => "production",
        })
    }
}

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

fn network() -> Result<Network> {
    match std::env::var("NETWORK").unwrap_or_default().as_str() {
        "testnet" => Ok(Network::Testnet),
        "mainnet" => Ok(Network::Mainnet),
        "" => bail!(
            "set NETWORK to testnet or mainnet. There is no default, because a default is \
             how a mainnet run happens by accident"
        ),
        other => bail!("NETWORK={other} is not a network this tool knows"),
    }
}

fn profile() -> Result<Profile> {
    match std::env::var("FLEET_PROFILE").unwrap_or_default().as_str() {
        "test" => Ok(Profile::Test),
        "production" => Ok(Profile::Production),
        "" => bail!(
            "set FLEET_PROFILE to test or production. It decides whether a fleet account \
             still holding a FullAccess key is a warning or a refusal"
        ),
        other => bail!("FLEET_PROFILE={other} is not a profile this tool knows"),
    }
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

const WALLET_ARTIFACT: &str = "hos_wallet";

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

enum Release {
    Tagged {
        name: String,
        commit: String,
        hashes: Vec<(String, String)>,
    },
    Planned {
        name: String,
        hashes: Vec<(String, String)>,
    },
}

impl Release {
    fn name(&self) -> &str {
        match self {
            Self::Tagged { name, .. } | Self::Planned { name, .. } => name,
        }
    }

    fn hashes(&self) -> &[(String, String)] {
        match self {
            Self::Tagged { hashes, .. } | Self::Planned { hashes, .. } => hashes,
        }
    }

    fn want(&self, tag_name: &str) -> Result<String> {
        self.hashes()
            .iter()
            .find(|(n, _)| n == tag_name)
            .map(|(_, h)| h.clone())
            .with_context(|| format!("{} names no artifact {tag_name}", self.name()))
    }

    fn describe(&self) -> String {
        match self {
            Self::Tagged { name, commit, .. } => format!("tag {name} -> {commit}"),
            Self::Planned { name, .. } => {
                format!("plan {name}, not bound to a source commit")
            }
        }
    }
}

fn manifest_path(root: &Path, name: &str) -> PathBuf {
    staged_dir(root, name).join("manifest.json")
}

fn resolve(root: &Path, name: &str) -> Result<Release> {
    if let Ok(commit) = tag_commit(name) {
        return Ok(Release::Tagged {
            name: name.to_string(),
            commit,
            hashes: tag_hashes(name)?,
        });
    }
    let path = manifest_path(root, name);
    let body = std::fs::read(&path).with_context(|| {
        format!(
            "{name} is neither a git tag nor a plan; {} does not exist. Run \
             `fleet plan {name}` after building, or name a tag",
            path.display()
        )
    })?;
    let doc: serde_json::Value = serde_json::from_slice(&body)?;
    let table = doc["artifacts"]
        .as_object()
        .with_context(|| format!("{} carries no artifact table", path.display()))?;
    let hashes = table
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|h| (k.clone(), h.to_string())))
        .collect();
    Ok(Release::Planned {
        name: name.to_string(),
        hashes,
    })
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

fn rpc(net: Network, body: serde_json::Value) -> Result<serde_json::Value> {
    let resp: serde_json::Value = ureq::post(net.rpc())
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())?
        .into_json()?;
    if !resp["error"].is_null() {
        bail!("{net} rpc refused the query: {}", resp["error"]);
    }
    Ok(resp)
}

fn chain_hash(net: Network, account: &str) -> Result<String> {
    let v = rpc(
        net,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "query",
            "params": {"request_type": "view_account", "finality": "final", "account_id": account}
        }),
    )?;
    v["result"]["code_hash"]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("no code_hash for {account} on {net}"))
}

fn view(net: Network, account: &str, method: &str) -> Result<serde_json::Value> {
    let v = rpc(
        net,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "query",
            "params": {"request_type": "call_function", "finality": "final",
                       "account_id": account, "method_name": method, "args_base64": "e30="}
        }),
    )?;
    if let Some(err) = v["result"]["error"].as_str() {
        bail!("{account}.{method} failed: {err}");
    }
    let raw = v["result"]["result"]
        .as_array()
        .with_context(|| format!("{account}.{method} returned nothing: {v}"))?;
    let bytes: Vec<u8> = raw
        .iter()
        .filter_map(|b| b.as_u64())
        .map(|b| b as u8)
        .collect();
    serde_json::from_slice(&bytes)
        .with_context(|| format!("{account}.{method} did not answer with json"))
}

fn view_if_present(net: Network, account: &str, method: &str) -> Result<Option<serde_json::Value>> {
    match view(net, account, method) {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.to_string().contains("MethodNotFound") => Ok(None),
        Err(e) => Err(e),
    }
}

fn probe(net: Network, account: &str, method: &str) -> Result<()> {
    view(net, account, method).map(|_| ())
}

fn staged_dir(root: &Path, name: &str) -> PathBuf {
    root.join("artifacts").join(name)
}

fn artifact_path(root: &Path, artifact: &str) -> PathBuf {
    root.join("target/near")
        .join(artifact)
        .join(format!("{artifact}.wasm"))
}

fn built_artifact(root: &Path, artifact: &str) -> Result<Vec<u8>> {
    let src = artifact_path(root, artifact);
    std::fs::read(&src).with_context(|| {
        format!(
            "read {}. Only `cargo near build` refreshes target/near; cargo build does not",
            src.display()
        )
    })
}

fn newest_source_change(root: &Path) -> SystemTime {
    let mut newest = SystemTime::UNIX_EPOCH;
    for dir in ["contracts", "crates"] {
        scan_sources(&root.join(dir), &mut newest);
    }
    newest
}

fn scan_sources(dir: &Path, newest: &mut SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_sources(&path, newest);
            continue;
        }
        let name = path.file_name().unwrap_or_default();
        let compiled_in = name == "Cargo.toml"
            || (path.extension().is_some_and(|e| e == "rs") && name != "tests.rs");
        if !compiled_in {
            continue;
        }
        if let Ok(changed) = entry.metadata().and_then(|m| m.modified()) {
            *newest = (*newest).max(changed);
        }
    }
}

fn assert_freshly_built(root: &Path, artifact: &str, newest_source: SystemTime) -> Result<()> {
    let path = artifact_path(root, artifact);
    let built_at = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .with_context(|| format!("stat {}", path.display()))?;
    if built_at < newest_source {
        bail!(
            "{} is older than the newest contract source. Nothing binds these bytes to a \
             commit, so a stale artifact would be planned, hashed and deployed as if it \
             were the code you just tested. Run cargo near build for every contract first",
            path.display()
        );
    }
    Ok(())
}

fn artifact_names() -> Vec<(String, String)> {
    FLEET
        .iter()
        .map(|t| (t.artifact.to_string(), t.tag_name.to_string()))
        .chain(std::iter::once((
            WALLET_ARTIFACT.to_string(),
            WALLET_ARTIFACT.replace('_', "-"),
        )))
        .collect()
}

fn plan(name: &str) -> Result<()> {
    let root = repo_root()?;
    let dest = staged_dir(&root, name);
    std::fs::create_dir_all(&dest)?;
    let mut table = serde_json::Map::new();
    let newest_source = newest_source_change(&root);

    for (artifact, tag_name) in artifact_names() {
        assert_freshly_built(&root, &artifact, newest_source)?;
        let bytes = built_artifact(&root, &artifact)?;
        let hash = bs58_of(&bytes);
        std::fs::write(dest.join(format!("{artifact}.wasm")), &bytes)?;
        println!("planned {tag_name:<22} {hash} ({} bytes)", bytes.len());
        table.insert(tag_name, serde_json::Value::String(hash));
    }

    let doc = serde_json::json!({ "name": name, "artifacts": table });
    std::fs::write(manifest_path(&root, name), serde_json::to_vec_pretty(&doc)?)?;
    println!(
        "\n{} artifact(s) planned at {}\nthese bytes carry no source-commit binding; \
         only a tagged release does",
        table_len(&doc),
        dest.display()
    );
    Ok(())
}

fn table_len(doc: &serde_json::Value) -> usize {
    doc["artifacts"].as_object().map_or(0, serde_json::Map::len)
}

fn stage(tag: &str) -> Result<()> {
    let root = repo_root()?;
    let commit = tag_commit(tag)?;
    let table = tag_hashes(tag)?;
    let dest = staged_dir(&root, tag);
    std::fs::create_dir_all(&dest)?;

    for (name, want) in &table {
        let artifact = name.replace('-', "_");
        let bytes = built_artifact(&root, &artifact)?;

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

fn load_staged(root: &Path, name: &str, artifact: &str) -> Result<Vec<u8>> {
    let path = staged_dir(root, name).join(format!("{artifact}.wasm"));
    std::fs::read(&path).with_context(|| {
        format!(
            "{} missing; run `fleet stage {name}` or `fleet plan {name}` first",
            path.display()
        )
    })
}

fn verify(net: Network, release: &Release) -> Result<bool> {
    let root = repo_root()?;
    let mut clean = true;

    println!("{} on {net}\n", release.describe());
    println!("{:<32} {:<46} state", "account", "on chain");
    for t in &FLEET {
        let want = release.want(t.tag_name)?;
        let bytes = load_staged(&root, release.name(), t.artifact)?;
        if bs58_of(&bytes) != want {
            bail!(
                "{}: staged artifact drifted from {}",
                t.account(),
                release.name()
            );
        }
        let on_chain = chain_hash(net, &t.account())?;
        let state = if on_chain == want {
            match probe(net, &t.account(), t.probe) {
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
        println!("{:<32} {:<46} {}", t.account(), on_chain, state);
    }
    Ok(clean)
}

fn signer() -> String {
    std::env::var("SIGN_WITH").unwrap_or_else(|_| "sign-with-legacy-keychain".to_string())
}

fn deploy_one(net: Network, t: &Target, path: &Path, want: &str) -> Result<()> {
    let previous = chain_hash(net, &t.account())?;
    if previous == want {
        println!("{:<32} already current, skipping", t.account());
        return Ok(());
    }
    println!("{:<32} {previous} -> {want}", t.account());

    let sign_with = signer();
    let status = Command::new("near")
        .args([
            "contract",
            "deploy",
            &t.account(),
            "use-file",
            path.to_str().context("artifact path is not utf8")?,
            "without-init-call",
            "network-config",
            net.cli_name(),
            &sign_with,
            "send",
        ])
        .status()?;
    if !status.success() {
        bail!("{}: near deploy exited {status}", t.account());
    }

    let now = chain_hash(net, &t.account())?;
    if now != want {
        bail!(
            "{}: deployed but chain reports {now}, expected {want}",
            t.account()
        );
    }
    if let Err(e) = probe(net, &t.account(), t.probe) {
        bail!(
            "{}: DEPLOYED BUT UNREADABLE ({e}). Roll back now with:\n  \
             near contract deploy {} use-file <artifact with hash {previous}> \
             without-init-call network-config {net} {sign_with} send",
            t.account(),
            t.account()
        );
    }
    println!("{:<32} deployed and readable", t.account());
    Ok(())
}

fn full_access_keys(net: Network, account: &str) -> Result<usize> {
    let v = rpc(
        net,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "query",
            "params": {"request_type": "view_access_key_list", "finality": "final",
                       "account_id": account}
        }),
    )?;
    let keys = v["result"]["keys"]
        .as_array()
        .with_context(|| format!("no key list for {account} on {net}"))?;
    Ok(keys
        .iter()
        .filter(|k| k["access_key"]["permission"] == "FullAccess")
        .count())
}

fn keys(net: Network, prof: Profile) -> Result<()> {
    let mut held_total = 0;
    for t in &FLEET {
        let held = full_access_keys(net, &t.account())?;
        held_total += held;
        let note = if held == 0 { "none" } else { "PRESENT" };
        println!("{:<32} {held} FullAccess key(s)  {note}", t.account());
    }
    if held_total == 0 {
        println!("\nno fleet account can be driven around its own contract");
        return Ok(());
    }
    let message = "a FullAccess key on a fleet account can deploy code and publish wallet \
                   bytes directly, skipping gd_approve and the approval delay, and every \
                   leased account follows the new code at once";
    match prof {
        Profile::Test => {
            println!("\n{held_total} FullAccess key(s) across the fleet: {message}.");
            println!("the test profile keeps them deliberately, so this is a note, not a gate");
            Ok(())
        }
        Profile::Production => bail!(
            "{held_total} FullAccess key(s) across the fleet: {message}. Until every one is \
             removed, the delay is a convention rather than a control"
        ),
    }
}

const READINESS_STEPS: [(&str, &str); 7] = [
    (
        "rate_set",
        "admin_set_initial_rate, then the oracle takes over",
    ),
    (
        "recovery_wired",
        "add_recovery_authority, council, one yocto",
    ),
    ("venue_set", "add_venue, council, one yocto"),
    (
        "ft_allowlist_set",
        "add_ft_allowlist once per settlement token, council, one yocto. Empty means the \
         balance gate never runs and a name changes hands carrying its tokens",
    ),
    ("metadata_set", "admin_set_nft_metadata, admin, one yocto"),
    (
        "wiring_sane",
        "fixed at construction; false means the registry points at itself",
    ),
    (
        "production_terms",
        "lease_term_ns and grace_period_ns are constructor arguments and cannot be raised later",
    ),
];

const GLOBAL_CODE_COST_PER_BYTE: u128 = 100_000_000_000_000_000_000;

const GLOBALS: [(&str, &str, &str); 1] = [("registrar", "registrar", "REGISTRAR_GLOBAL_ACCOUNT")];

fn global_hash(net: Network, account: &str) -> Result<Option<String>> {
    let answer = rpc(
        net,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "query",
            "params": {"request_type": "view_global_contract_code_by_account_id",
                       "finality": "final", "account_id": account}
        }),
    );
    let v = match answer {
        Ok(v) => v,
        Err(e) if e.to_string().contains("NO_GLOBAL_CONTRACT_CODE") => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(v["result"]["hash"].as_str().map(str::to_string))
}

fn balance_yocto(net: Network, account: &str) -> Result<u128> {
    let v = rpc(
        net,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "query",
            "params": {"request_type": "view_account", "finality": "final", "account_id": account}
        }),
    )?;
    v["result"]["amount"]
        .as_str()
        .and_then(|a| a.parse().ok())
        .with_context(|| format!("no balance for {account} on {net}"))
}

fn near_of(yocto: u128) -> String {
    format!("{:.2}", yocto as f64 / 1e24)
}

fn publish(net: Network, release: &Release, tag_name: &str) -> Result<()> {
    let (_, artifact, var) = GLOBALS
        .iter()
        .find(|(name, _, _)| *name == tag_name)
        .with_context(|| format!("{tag_name} is not published as a global contract"))?;
    let publisher = account(var);
    let root = repo_root()?;
    let bytes = load_staged(&root, release.name(), artifact)?;
    let want = release.want(tag_name)?;
    let staged = bs58_of(&bytes);
    if staged != want {
        bail!(
            "staged {artifact} hashes {staged}, {} names {want}",
            release.name()
        );
    }

    println!("{artifact} {} bytes, hash {staged}", bytes.len());
    println!("publisher {publisher} on {net}, from {var}");
    println!(
        "this signs DeployGlobalContract with the publisher's own key. It does not touch \
         gd_approve or gd_deploy, so the council delay does not apply. That is the genesis \
         path; once the publisher holds no key this command can no longer run and every \
         later publish goes through the council."
    );

    if global_hash(net, &publisher)?.as_deref() == Some(want.as_str()) {
        println!("already published, nothing to do");
        return Ok(());
    }

    let cost = (bytes.len() as u128).saturating_mul(GLOBAL_CODE_COST_PER_BYTE);
    let held = balance_yocto(net, &publisher)?;
    println!(
        "costs {} NEAR, never refunded, charged again on every republish",
        near_of(cost)
    );
    println!("publisher holds {} NEAR", near_of(held));
    if held < cost {
        bail!(
            "{publisher} is short {} NEAR",
            near_of(cost.saturating_sub(held))
        );
    }

    let path = staged_dir(&root, release.name()).join(format!("{artifact}.wasm"));
    let sign_with = signer();
    let status = Command::new("near")
        .args([
            "contract",
            "deploy-as-global",
            "use-file",
            path.to_str().context("artifact path is not utf8")?,
            "as-global-account-id",
            &publisher,
            "network-config",
            net.cli_name(),
            &sign_with,
            "send",
        ])
        .status()?;
    if !status.success() {
        bail!("{publisher}: near deploy-as-global exited {status}");
    }

    match global_hash(net, &publisher)? {
        Some(now) if now == want => {
            println!("\npublished {want} as a global under {publisher}");
            println!(
                "point an account at it with `near contract deploy ACCOUNT \
                 use-global-account-id {publisher} with-init-call`, naming the init \
                 method and its json args, on network-config {net} with {sign_with}"
            );
            Ok(())
        }
        Some(now) => bail!("published but chain reports {now}, expected {want}"),
        None => bail!("near reported success but no global is published under {publisher}"),
    }
}

const DAY_NS: u64 = 24 * 60 * 60 * 1_000_000_000;
const PRODUCTION_GRACE_NS: u64 = 14 * DAY_NS;

fn derived(bad: &mut Vec<String>, label: &str, got: &serde_json::Value, want: &str) {
    let got = got.as_str().unwrap_or("<absent>");
    let verdict = if got == want {
        "ok"
    } else {
        bad.push(format!("{label} is {got}, the fleet layout says {want}"));
        "MISMATCH"
    };
    println!("{label:<30} {got:<34} {verdict}");
}

fn chosen(bad: &mut Vec<String>, label: &str, value: &str) {
    if value == "<absent>" || value == "null" {
        bad.push(format!("{label} did not answer"));
        println!("{label:<30} {value:<34} UNREADABLE");
        return;
    }
    println!("{label:<30} {value:<34} confirm against intent");
}

fn permanence(net: Network) -> Result<()> {
    let registry = account("REGISTRY_ACCOUNT");
    let extension = account("EXTENSION_ACCOUNT");
    let recovery = account("RECOVERY_ACCOUNT");
    let root = account("ROOT_ACCOUNT");
    let deployer = account("DEPLOYER_ACCOUNT");

    let root_config = view(net, &root, "config")?;
    let deployer_config = view(net, &deployer, "config")?;
    let registry_treasury = view(net, &registry, "get_treasury")?;
    let extension_treasury = view_if_present(net, &extension, "get_treasury")?;
    let grace = view(net, &registry, "get_grace_period_ns")?;

    let mut bad = Vec::new();
    println!("no setter exists for the derived or chosen fields. a wrong value there means");
    println!("redeploying that contract. treasury is the exception: both contracts rotate it");
    println!("behind a council approval and a 48h delay, and both must be rotated together.\n");
    println!("{:<30} {:<34} state", "field", "on chain");

    println!("\n-- derived from the fleet layout");
    derived(
        &mut bad,
        "registry.hos_extension",
        &view(net, &registry, "get_hos_extension")?,
        &extension,
    );
    derived(
        &mut bad,
        "extension.registry",
        &view(net, &extension, "get_registry")?,
        &registry,
    );
    derived(
        &mut bad,
        "extension.recovery",
        &view(net, &extension, "get_recovery")?,
        &recovery,
    );
    derived(
        &mut bad,
        "registrar.registry",
        &root_config["registry"],
        &registry,
    );
    derived(
        &mut bad,
        "registrar.hos_extension",
        &root_config["hos_extension"],
        &extension,
    );
    derived(
        &mut bad,
        "registrar.recovery",
        &root_config["recovery"],
        &recovery,
    );
    derived(
        &mut bad,
        "registrar.wallet_impl",
        &root_config["wallet_impl"],
        &deployer,
    );
    derived(
        &mut bad,
        "recovery.transfer_authority",
        &view(net, &recovery, "transfer_authority")?,
        &extension,
    );
    derived(
        &mut bad,
        "registrar.chain_id",
        &root_config["chain_id"],
        net.cli_name(),
    );

    println!("\n-- the same value must appear in both places");
    let registry_treasury = registry_treasury.as_str().unwrap_or("<absent>");
    chosen(&mut bad, "registry.treasury", registry_treasury);
    match extension_treasury.as_ref().and_then(|v| v.as_str()) {
        Some(found) if found == registry_treasury => println!(
            "{:<30} {:<34} matches the registry",
            "extension.treasury", found
        ),
        Some(found) => {
            bad.push(format!(
                "extension.treasury is {found}, the registry pays {registry_treasury}"
            ));
            println!("{:<30} {:<34} DIVERGED", "extension.treasury", found);
        }
        None => {
            bad.push(
                "extension.treasury cannot be read; this build predates get_treasury".to_string(),
            );
            println!(
                "{:<30} {:<34} NO GETTER",
                "extension.treasury", "unreadable"
            );
        }
    }

    for (label, account) in [
        ("registry.pending_treasury", &registry),
        ("extension.pending_treasury", &extension),
    ] {
        match view_if_present(net, account, "pending_treasury")? {
            Some(serde_json::Value::Null) | None => {
                println!("{label:<30} {:<34} none in flight", "-")
            }
            Some(pending) => {
                let to = pending[0].as_str().unwrap_or("<unreadable>");
                bad.push(format!(
                    "{label} is mid-rotation to {to}; commit or cancel it on both contracts \
                     before trusting the treasury above"
                ));
                println!("{label:<30} {to:<34} ROTATION IN FLIGHT");
            }
        }
    }

    println!("\n-- chosen, nothing on chain can check these for you");
    chosen(
        &mut bad,
        "recovery.owner",
        view(net, &recovery, "owner")?
            .as_str()
            .unwrap_or("<absent>"),
    );
    chosen(
        &mut bad,
        "recovery.signer",
        view(net, &recovery, "signer")?
            .as_str()
            .unwrap_or("<absent>"),
    );
    chosen(
        &mut bad,
        "registrar.wallet_timeout_secs",
        &root_config["wallet_timeout_secs"].to_string(),
    );

    println!("\n-- global contract code the fleet points at");
    for (tag, _, var) in &GLOBALS {
        let Ok(publisher) = std::env::var(var) else {
            println!("{:<30} {:<34} {var} unset, not checked", *tag, "-");
            continue;
        };
        match global_hash(net, &publisher)? {
            Some(hash) => println!("{:<30} {hash:<34} published", *tag),
            None => {
                bad.push(format!("{tag}: nothing published under {publisher}"));
                println!("{:<30} {:<34} NOTHING PUBLISHED", *tag, publisher);
            }
        }
    }

    println!("\n-- delays");
    match grace.as_str().and_then(|g| g.parse::<u64>().ok()) {
        Some(ns) if ns >= PRODUCTION_GRACE_NS => println!(
            "{:<30} {:<34} ok",
            "registry.grace_period_ns",
            format!("{} days", ns / DAY_NS)
        ),
        Some(ns) => {
            bad.push(format!(
                "grace_period_ns is {} days, production wants 14",
                ns / DAY_NS
            ));
            println!(
                "{:<30} {:<34} TOO SHORT",
                "registry.grace_period_ns",
                format!("{} days", ns / DAY_NS)
            );
        }
        None => {
            bad.push("grace_period_ns did not answer with a number".to_string());
            println!("{:<30} {:<34} UNREADABLE", "registry.grace_period_ns", "-");
        }
    }
    if deployer_config["production_delay"].as_bool() == Some(true) {
        println!(
            "{:<30} {:<34} ok",
            "deployer.approval_delay_ns", "48h or longer"
        );
    } else {
        bad.push("deployer approval_delay_ns is below the 48h production delay".to_string());
        println!(
            "{:<30} {:<34} TOO SHORT",
            "deployer.approval_delay_ns", "under 48h"
        );
    }

    if bad.is_empty() {
        println!("\nevery permanent field agrees with the fleet layout");
        return Ok(());
    }
    for line in &bad {
        println!("\n  {line}");
    }
    bail!(
        "{} permanent field(s) wrong, fix before deleting any key",
        bad.len()
    )
}

fn ready(net: Network, prof: Profile) -> Result<()> {
    let registry = account("REGISTRY_ACCOUNT");
    let state = view(net, &registry, "deployment_readiness")?;
    let mut missing = Vec::new();

    println!("{registry} on {net}\n");
    for (flag, remedy) in READINESS_STEPS {
        let verdict = match state[flag].as_bool() {
            Some(true) => "ok",
            Some(false) => "MISSING",
            None => "UNKNOWN, the deployed registry predates this check",
        };
        println!("{flag:<20} {verdict}");
        if verdict != "ok" {
            missing.push(flag);
            println!("{:<20}   {remedy}", "");
        }
    }
    println!(
        "\nlease_term_ns {} grace_period_ns {}",
        state["lease_term_ns"], state["grace_period_ns"]
    );

    if missing.is_empty() {
        println!("registry reports ready");
        return Ok(());
    }
    match prof {
        Profile::Test => {
            println!(
                "\n{} step(s) outstanding, reported not refused",
                missing.len()
            );
            Ok(())
        }
        Profile::Production => bail!("{} deployment step(s) outstanding", missing.len()),
    }
}

fn deploy(net: Network, prof: Profile, release: &Release) -> Result<()> {
    let root = repo_root()?;
    keys(net, prof)?;
    println!();
    verify(net, release).ok();
    println!();

    for t in &FLEET {
        let want = release.want(t.tag_name)?;
        let path = staged_dir(&root, release.name()).join(format!("{}.wasm", t.artifact));
        let bytes = load_staged(&root, release.name(), t.artifact)?;
        if bs58_of(&bytes) != want {
            bail!(
                "{}: staged artifact does not match {}",
                t.account(),
                release.name()
            );
        }
        deploy_one(net, t, &path, &want)?;
    }

    println!();
    if verify(net, release)? {
        println!(
            "\nfleet matches {} and every contract answers",
            release.name()
        );
    } else {
        bail!("fleet does not match {} after deploying", release.name());
    }
    ready(net, prof)
}

fn up(net: Network, prof: Profile, name: &str) -> Result<()> {
    let root = repo_root()?;
    println!("== stage");
    match resolve(&root, name)? {
        Release::Tagged { .. } => stage(name)?,
        Release::Planned { .. } => plan(name)?,
    }
    let release = resolve(&root, name)?;
    println!("\n== keys");
    keys(net, prof)?;
    println!("\n== deploy");
    deploy(net, prof, &release)
}

fn usage() -> ! {
    let tags: Vec<&str> = GLOBALS.iter().map(|(tag, _, _)| *tag).collect();
    eprintln!(
        "usage: fleet <up|plan|stage|verify|keys|ready|permanence|deploy> [release]\n       \
         fleet publish <release> <{}>",
        tags.join("|")
    );
    eprintln!("  NETWORK=testnet|mainnet  FLEET_PROFILE=test|production");
    eprintln!("  SIGN_WITH=sign-with-legacy-keychain (default) | sign-with-keychain");
    std::process::exit(2)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let Some(cmd) = args.get(1) else { usage() };
    let net = network()?;
    let prof = profile()?;
    println!("network {net}, profile {prof}\n");

    let release = |name: Option<&String>| -> Result<Release> {
        let Some(name) = name else { usage() };
        resolve(&repo_root()?, name)
    };

    match cmd.as_str() {
        "keys" => keys(net, prof),
        "ready" => ready(net, prof),
        "permanence" => permanence(net),
        "publish" => match (args.get(2), args.get(3)) {
            (Some(name), Some(tag)) => publish(net, &resolve(&repo_root()?, name)?, tag),
            _ => usage(),
        },
        "plan" => match args.get(2) {
            Some(name) => plan(name),
            None => usage(),
        },
        "stage" => match args.get(2) {
            Some(name) => stage(name),
            None => usage(),
        },
        "up" => match args.get(2) {
            Some(name) => up(net, prof, name),
            None => usage(),
        },
        "verify" => {
            if !verify(net, &release(args.get(2))?)? {
                std::process::exit(1);
            }
            Ok(())
        }
        "deploy" => deploy(net, prof, &release(args.get(2))?),
        other => {
            eprintln!("unknown command {other}");
            usage()
        }
    }
}
