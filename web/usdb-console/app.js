const i18n = window.USDBConsoleI18n;
let currentLocale = i18n.resolveLocale();

const els = {
    refreshBtn: document.getElementById("refresh-btn"),
    localeSelect: document.getElementById("locale-select"),
    updatedAt: document.getElementById("metric-updated-at"),
    btcNetwork: document.getElementById("metric-btc-network"),
    btcHeight: document.getElementById("metric-btc-height"),
    ethwHeight: document.getElementById("metric-ethw-height"),
    servicesSummary: document.getElementById("services-summary"),
    bootstrapOverallState: document.getElementById("bootstrap-overall-state"),
    bootstrapSteps: document.getElementById("bootstrap-steps"),
    linkBalanceHistory: document.getElementById("link-balance-history"),
    linkUsdbIndexer: document.getElementById("link-usdb-indexer"),
    bootstrapManifest: makeArtifactEls("bootstrap-manifest"),
    snapshotMarker: makeArtifactEls("snapshot-marker"),
    ethwMarker: makeArtifactEls("ethw-marker"),
    services: {
        btcNode: makeServiceEls("service-btc-node"),
        balanceHistory: makeServiceEls("service-balance-history"),
        usdbIndexer: makeServiceEls("service-usdb-indexer"),
        ethw: makeServiceEls("service-ethw"),
    },
};

function t(key, params) {
    return i18n.t(currentLocale, key, params);
}

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

function applyStaticTranslations() {
    document.documentElement.lang = currentLocale;
    els.localeSelect.value = currentLocale;
    for (const element of document.querySelectorAll("[data-i18n]")) {
        element.textContent = t(element.dataset.i18n);
    }
}

function setLocale(locale) {
    currentLocale = i18n.normalizeLocale(locale);
    window.localStorage.setItem("usdb-console.locale", currentLocale);
    applyStaticTranslations();
    void refresh();
}

function fmtDate(ms) {
    if (!ms) return t("common.none");
    return new Date(ms).toLocaleString(currentLocale, { hour12: false });
}

function fmtNum(value) {
    if (value === null || value === undefined || Number.isNaN(Number(value))) {
        return t("common.none");
    }
    return new Intl.NumberFormat(currentLocale).format(Number(value));
}

function shortText(value, head = 14, tail = 12) {
    const text = String(value ?? "");
    if (!text) return t("common.none");
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
        val.textContent = value ?? t("common.none");
        row.append(key, val);
        container.append(row);
    }
}

function renderServiceCard(target, probe, detailsBuilder) {
    target.rpcUrl.textContent = probe.rpc_url || t("common.none");
    target.error.textContent = probe.error || "";
    if (!probe.reachable) {
        setPill(target.pill, t("serviceStates.offline"), "bad");
        renderDetailGrid(target.details, [
            [t("fields.latency"), probe.latency_ms ? `${probe.latency_ms} ms` : t("common.none")],
        ]);
        return;
    }

    const tone = probe.data?.consensus_ready ? "ok" : probe.data?.query_ready ? "warn" : "warn";
    const label = probe.data?.consensus_ready
        ? t("serviceStates.consensusReady")
        : probe.data?.query_ready
            ? t("serviceStates.queryReady")
            : t("serviceStates.reachable");
    setPill(target.pill, label, tone);
    renderDetailGrid(target.details, detailsBuilder(probe));
}

function renderArtifact(target, summary) {
    target.path.textContent = summary.path || t("common.none");
    target.error.textContent = summary.error || "";
    if (!summary.exists) {
        setPill(target.pill, t("artifactStates.missing"), "bad");
        target.details.innerHTML = "";
        return;
    }

    setPill(target.pill, t("artifactStates.present"), "ok");
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
        overview.services.btc_node.data?.chain ||
        overview.services.balance_history.data?.network ||
        overview.services.usdb_indexer.data?.network ||
        t("common.none");
    els.btcHeight.textContent = fmtNum(overview.services.btc_node.data?.blocks);
    els.ethwHeight.textContent = fmtNum(overview.services.ethw.data?.block_number);

    const totalServices = 4;
    const readyCount = [
        overview.services.btc_node,
        overview.services.balance_history,
        overview.services.usdb_indexer,
        overview.services.ethw,
    ].filter((service) => service.reachable).length;
    els.servicesSummary.textContent = t("services.summary", {
        readyCount,
        total: totalServices,
    });

    renderServiceCard(els.services.btcNode, overview.services.btc_node, (probe) => [
        [t("fields.chain"), probe.data?.chain || t("common.none")],
        [t("fields.blocks"), fmtNum(probe.data?.blocks)],
        [t("fields.headers"), fmtNum(probe.data?.headers)],
        [
            t("fields.ibd"),
            probe.data?.initial_block_download === undefined
                ? t("common.none")
                : String(probe.data.initial_block_download),
        ],
        [
            t("fields.verifyProgress"),
            probe.data?.verification_progress === undefined
                ? t("common.none")
                : `${(probe.data.verification_progress * 100).toFixed(2)}%`,
        ],
        [t("fields.latency"), probe.latency_ms ? `${probe.latency_ms} ms` : t("common.none")],
    ]);

    renderServiceCard(els.services.balanceHistory, overview.services.balance_history, (probe) => [
        [t("fields.network"), probe.data?.network || t("common.none")],
        [t("fields.stableHeight"), fmtNum(probe.data?.stable_height)],
        [t("fields.phase"), probe.data?.phase || t("common.none")],
        [t("fields.consensus"), String(Boolean(probe.data?.consensus_ready))],
        [t("fields.snapshotVerify"), probe.data?.snapshot_verification_state || t("common.none")],
        [t("fields.blockers"), probe.data?.blockers?.join(", ") || t("common.none")],
    ]);

    renderServiceCard(els.services.usdbIndexer, overview.services.usdb_indexer, (probe) => [
        [t("fields.network"), probe.data?.network || t("common.none")],
        [t("fields.syncedHeight"), fmtNum(probe.data?.synced_block_height)],
        [t("fields.stableHeight"), fmtNum(probe.data?.balance_history_stable_height)],
        [t("fields.consensus"), String(Boolean(probe.data?.consensus_ready))],
        [t("fields.systemState"), shortText(probe.data?.system_state_id || t("common.none"))],
        [t("fields.blockers"), probe.data?.blockers?.join(", ") || t("common.none")],
    ]);

    renderServiceCard(els.services.ethw, overview.services.ethw, (probe) => [
        [t("fields.client"), probe.data?.client_version || t("common.none")],
        [t("fields.chainId"), probe.data?.chain_id || t("common.none")],
        [t("fields.networkId"), probe.data?.network_id || t("common.none")],
        [t("fields.blockNumber"), fmtNum(probe.data?.block_number)],
        [
            t("fields.syncing"),
            probe.data?.syncing === false
                ? t("common.false")
                : JSON.stringify(probe.data?.syncing ?? t("common.none")),
        ],
        [t("fields.latency"), probe.latency_ms ? `${probe.latency_ms} ms` : t("common.none")],
    ]);

    renderArtifact(els.bootstrapManifest, overview.bootstrap.bootstrap_manifest);
    renderArtifact(els.snapshotMarker, overview.bootstrap.snapshot_marker);
    renderArtifact(els.ethwMarker, overview.bootstrap.ethw_init_marker);
    renderBootstrapSteps(overview.bootstrap);

    els.linkBalanceHistory.href = overview.explorers.balance_history;
    els.linkUsdbIndexer.href = overview.explorers.usdb_indexer;
}

function renderBootstrapSteps(bootstrap) {
    const tone =
        bootstrap.overall_state === "completed" ? "ok" :
        bootstrap.overall_state === "error" ? "bad" :
        bootstrap.overall_state === "in_progress" ? "warn" : "warn";
    setPill(
        els.bootstrapOverallState,
        t(`states.${bootstrap.overall_state}`),
        tone,
    );
    els.bootstrapSteps.innerHTML = "";
    for (const step of bootstrap.steps || []) {
        const card = document.createElement("article");
        card.className = "step-card";
        const head = document.createElement("div");
        head.className = "service-head";
        const title = document.createElement("h3");
        title.textContent = t(`bootstrap.steps.${step.step}`);
        const pill = document.createElement("span");
        const stepTone =
            step.state === "completed" ? "ok" :
            step.state === "error" ? "bad" :
            step.state === "in_progress" ? "warn" : "warn";
        setPill(pill, t(`states.${step.state}`), stepTone);
        head.append(title, pill);
        const detail = document.createElement("p");
        const detailKey =
            step.state === "completed"
                ? "bootstrap.stepDetails.completed"
                : step.state === "error"
                    ? "bootstrap.stepDetails.error"
                    : "bootstrap.stepDetails.pending";
        detail.textContent = t(detailKey, {
            label: t(`bootstrap.steps.${step.step}`),
            path: step.artifact_path || t("common.none"),
            error: step.error || t("common.none"),
        });
        card.append(head, detail);
        els.bootstrapSteps.append(card);
    }
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

els.refreshBtn.addEventListener("click", () => {
    void refresh();
});

els.localeSelect.addEventListener("change", (event) => {
    setLocale(event.target.value);
});

applyStaticTranslations();
void refresh();
window.setInterval(() => {
    void refresh();
}, 8000);
