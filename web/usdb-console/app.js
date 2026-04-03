const els = {
    refreshBtn: document.getElementById("refresh-btn"),
    updatedAt: document.getElementById("metric-updated-at"),
    btcNetwork: document.getElementById("metric-btc-network"),
    stableHeight: document.getElementById("metric-stable-height"),
    ethwHeight: document.getElementById("metric-ethw-height"),
    servicesSummary: document.getElementById("services-summary"),
    linkBalanceHistory: document.getElementById("link-balance-history"),
    linkUsdbIndexer: document.getElementById("link-usdb-indexer"),
    bootstrapManifest: makeArtifactEls("bootstrap-manifest"),
    snapshotMarker: makeArtifactEls("snapshot-marker"),
    ethwMarker: makeArtifactEls("ethw-marker"),
    services: {
        balanceHistory: makeServiceEls("service-balance-history"),
        usdbIndexer: makeServiceEls("service-usdb-indexer"),
        ethw: makeServiceEls("service-ethw"),
    },
};

function makeServiceEls(id) {
    const root = document.getElementById(id);
    return {
        root,
        pill: root.querySelector('[data-kind="state"]'),
        rpcUrl: root.querySelector('[data-field="rpc-url"]'),
        details: root.querySelector('[data-field="details"]'),
        error: root.querySelector('[data-field="error"]'),
    };
}

function makeArtifactEls(prefix) {
    return {
        pill: document.getElementById(`${prefix}-state`),
        path: document.getElementById(`${prefix}-path`),
        details: document.getElementById(`${prefix}-details`),
        error: document.getElementById(`${prefix}-error`),
    };
}

function fmtDate(ms) {
    if (!ms) return "-";
    return new Date(ms).toLocaleString("zh-CN", { hour12: false });
}

function fmtNum(value) {
    if (value === null || value === undefined || Number.isNaN(Number(value))) return "-";
    return new Intl.NumberFormat("en-US").format(Number(value));
}

function shortText(value, head = 14, tail = 12) {
    const text = String(value ?? "");
    if (!text) return "-";
    if (text.length <= head + tail + 3) return text;
    return `${text.slice(0, head)}...${text.slice(-tail)}`;
}

function setPill(el, text, tone) {
    el.textContent = text;
    el.classList.remove("ok", "warn", "bad");
    if (tone) el.classList.add(tone);
}

function renderDetailGrid(container, entries) {
    container.innerHTML = "";
    for (const [label, value] of entries) {
        const row = document.createElement("div");
        const key = document.createElement("span");
        const val = document.createElement("strong");
        key.textContent = label;
        val.textContent = value ?? "-";
        row.append(key, val);
        container.append(row);
    }
}

function renderServiceCard(target, probe, detailsBuilder) {
    target.rpcUrl.textContent = probe.rpc_url || "-";
    target.error.textContent = probe.error || "";
    if (!probe.reachable) {
        setPill(target.pill, "offline", "bad");
        renderDetailGrid(target.details, [
            ["Latency", probe.latency_ms ? `${probe.latency_ms} ms` : "-"],
        ]);
        return;
    }

    const tone = probe.data?.consensus_ready ? "ok" : probe.data?.query_ready ? "warn" : "warn";
    const label = probe.data?.consensus_ready ? "consensus ready" : probe.data?.query_ready ? "query ready" : "reachable";
    setPill(target.pill, label, tone);
    renderDetailGrid(target.details, detailsBuilder(probe));
}

function renderArtifact(target, summary) {
    target.path.textContent = summary.path || "-";
    target.error.textContent = summary.error || "";
    if (!summary.exists) {
        setPill(target.pill, "missing", "bad");
        target.details.innerHTML = "";
        return;
    }

    setPill(target.pill, "present", "ok");
    const data = summary.data || {};
    const entries = Object.entries(data)
        .slice(0, 8)
        .map(([key, value]) => [key, typeof value === "object" ? JSON.stringify(value) : String(value)]);
    renderDetailGrid(target.details, entries);
}

async function fetchOverview() {
    const resp = await fetch("/api/system/overview", { cache: "no-store" });
    if (!resp.ok) {
        throw new Error(`Failed to load overview: HTTP ${resp.status}`);
    }
    return resp.json();
}

function renderOverview(overview) {
    els.updatedAt.textContent = fmtDate(overview.generated_at_ms);
    els.btcNetwork.textContent =
        overview.services.balance_history.data?.network ||
        overview.services.usdb_indexer.data?.network ||
        "-";
    els.stableHeight.textContent = fmtNum(overview.services.balance_history.data?.stable_height);
    els.ethwHeight.textContent = fmtNum(overview.services.ethw.data?.block_number);

    const readyCount = [
        overview.services.balance_history,
        overview.services.usdb_indexer,
        overview.services.ethw,
    ].filter((service) => service.reachable).length;
    els.servicesSummary.textContent = `当前 ${readyCount}/3 个核心服务可达；首页优先展示 readiness、bootstrap 与 explorer 入口。`;

    renderServiceCard(els.services.balanceHistory, overview.services.balance_history, (probe) => [
        ["Network", probe.data?.network || "-"],
        ["Stable Height", fmtNum(probe.data?.stable_height)],
        ["Phase", probe.data?.phase || "-"],
        ["Consensus", String(Boolean(probe.data?.consensus_ready))],
        ["Snapshot Verify", probe.data?.snapshot_verification_state || "-"],
        ["Blockers", probe.data?.blockers?.join(", ") || "-"],
    ]);

    renderServiceCard(els.services.usdbIndexer, overview.services.usdb_indexer, (probe) => [
        ["Network", probe.data?.network || "-"],
        ["Synced Height", fmtNum(probe.data?.synced_block_height)],
        ["Stable Height", fmtNum(probe.data?.balance_history_stable_height)],
        ["Consensus", String(Boolean(probe.data?.consensus_ready))],
        ["System State", shortText(probe.data?.system_state_id || "-")],
        ["Blockers", probe.data?.blockers?.join(", ") || "-"],
    ]);

    renderServiceCard(els.services.ethw, overview.services.ethw, (probe) => [
        ["Client", probe.data?.client_version || "-"],
        ["Chain ID", probe.data?.chain_id || "-"],
        ["Network ID", probe.data?.network_id || "-"],
        ["Block Number", fmtNum(probe.data?.block_number)],
        ["Syncing", probe.data?.syncing === false ? "false" : JSON.stringify(probe.data?.syncing ?? "-")],
        ["Latency", probe.latency_ms ? `${probe.latency_ms} ms` : "-"],
    ]);

    renderArtifact(els.bootstrapManifest, overview.bootstrap.bootstrap_manifest);
    renderArtifact(els.snapshotMarker, overview.bootstrap.snapshot_marker);
    renderArtifact(els.ethwMarker, overview.bootstrap.ethw_init_marker);

    els.linkBalanceHistory.href = overview.explorers.balance_history;
    els.linkUsdbIndexer.href = overview.explorers.usdb_indexer;
}

async function refresh() {
    els.refreshBtn.disabled = true;
    try {
        const overview = await fetchOverview();
        renderOverview(overview);
    } catch (error) {
        console.error(error);
        els.servicesSummary.textContent = error.message;
    } finally {
        els.refreshBtn.disabled = false;
    }
}

els.refreshBtn.addEventListener("click", refresh);
refresh();
window.setInterval(refresh, 8000);
