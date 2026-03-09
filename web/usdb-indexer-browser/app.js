const state = {
    rpcUrl: "http://127.0.0.1:28020",
    homeRefreshMs: 5000,
    homeTimer: null,
    clockTimer: null,
    pass: {
        inscriptionId: "",
        atHeight: null,
        page: 0,
        pageSize: 20,
        order: "desc",
        total: 0,
        fromHeight: 0,
        toHeight: 0,
    },
    energy: {
        leaderboard: {
            scope: "active",
            page: 0,
            pageSize: 50,
            total: 0,
        },
        selectedInscriptionId: "",
        latestSnapshot: null,
        range: {
            fromHeight: null,
            toHeight: null,
            page: 0,
            pageSize: 50,
            total: 0,
            order: "desc",
        },
    },
};

const RPC_DEFAULT_ENDPOINTS = {
    mainnet: "http://127.0.0.1:28020",
    regtest: "http://127.0.0.1:28120",
    testnet: "http://127.0.0.1:28220",
    signet: "http://127.0.0.1:28320",
    testnet4: "http://127.0.0.1:28420",
};

const els = {
    rpcForm: document.getElementById("rpc-form"),
    rpcNetwork: document.getElementById("rpc-network"),
    rpcUrl: document.getElementById("rpc-url"),
    rpcHint: document.getElementById("rpc-hint"),
    tabs: Array.from(document.querySelectorAll(".tab")),
    panels: Array.from(document.querySelectorAll(".panel")),

    homeNow: document.getElementById("home-now"),
    homeNetwork: document.getElementById("home-network"),
    homeSyncedHeight: document.getElementById("home-synced-height"),
    homeDependHeight: document.getElementById("home-depend-height"),
    homeActivePass: document.getElementById("home-active-pass"),
    homeTotalPass: document.getElementById("home-total-pass"),
    homeActiveBalance: document.getElementById("home-active-balance"),
    homeSyncMessage: document.getElementById("home-sync-message"),
    homeSyncProgress: document.getElementById("home-sync-progress"),
    homeSyncCurrent: document.getElementById("home-sync-current"),
    homeSyncTotal: document.getElementById("home-sync-total"),
    homeRpcLatency: document.getElementById("home-rpc-latency"),
    homeUpdatedAt: document.getElementById("home-updated-at"),
    homeError: document.getElementById("home-error"),
    homeRefresh: document.getElementById("home-refresh"),

    passQueryForm: document.getElementById("pass-query-form"),
    passIdInput: document.getElementById("pass-id-input"),
    passHeightInput: document.getElementById("pass-height-input"),
    passQueryHint: document.getElementById("pass-query-hint"),
    passSnapshotEmpty: document.getElementById("pass-snapshot-empty"),
    passSnapshotBox: document.getElementById("pass-snapshot-box"),
    passSnapshotGrid: document.getElementById("pass-snapshot-grid"),
    passHistoryPrev: document.getElementById("pass-history-prev"),
    passHistoryNext: document.getElementById("pass-history-next"),
    passHistoryPage: document.getElementById("pass-history-page"),
    passHistorySummary: document.getElementById("pass-history-summary"),
    passHistoryTable: document.getElementById("pass-history-table"),
    passHistoryError: document.getElementById("pass-history-error"),

    energyLeaderboardPrev: document.getElementById("energy-leaderboard-prev"),
    energyLeaderboardNext: document.getElementById("energy-leaderboard-next"),
    energyLeaderboardScope: document.getElementById("energy-leaderboard-scope"),
    energyLeaderboardPage: document.getElementById("energy-leaderboard-page"),
    energyLeaderboardSummary: document.getElementById("energy-leaderboard-summary"),
    energyLeaderboardTable: document.getElementById("energy-leaderboard-table"),
    energyQueryForm: document.getElementById("energy-query-form"),
    energyIdInput: document.getElementById("energy-id-input"),
    energyQueryHint: document.getElementById("energy-query-hint"),
    energySnapshotEmpty: document.getElementById("energy-snapshot-empty"),
    energySnapshotBox: document.getElementById("energy-snapshot-box"),
    energySnapshotGrid: document.getElementById("energy-snapshot-grid"),
    energyRangeForm: document.getElementById("energy-range-form"),
    energyRangeFrom: document.getElementById("energy-range-from"),
    energyRangeTo: document.getElementById("energy-range-to"),
    energyRangePrev: document.getElementById("energy-range-prev"),
    energyRangeNext: document.getElementById("energy-range-next"),
    energyRangePage: document.getElementById("energy-range-page"),
    energyRangeSummary: document.getElementById("energy-range-summary"),
    energyRangeTable: document.getElementById("energy-range-table"),
    energyRangeError: document.getElementById("energy-range-error"),
};

function fmtNum(value) {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    return new Intl.NumberFormat("en-US").format(value);
}

function shortText(value, head = 14, tail = 12) {
    const text = String(value ?? "");
    if (!text) return "-";
    if (text.length <= head + tail + 3) return text;
    return `${text.slice(0, head)}...${text.slice(-tail)}`;
}

function toNumber(value) {
    if (value === null || value === undefined || value === "") return null;
    const n = Number(value);
    return Number.isFinite(n) ? n : null;
}

function fmtBtc(value) {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    let text = Number(value).toFixed(8);
    text = text.replace(/\.?0+$/, "");
    return text;
}

function fmtBalanceSmart(valueSat) {
    const sat = toNumber(valueSat);
    if (sat === null) return "-";
    const abs = Math.abs(sat);
    if (abs >= 100_000_000) {
        return `${fmtBtc(sat / 100_000_000)} BTC`;
    }
    return `${fmtNum(sat)} sat`;
}

function fmtBalanceDeltaSmart(valueSat) {
    const sat = toNumber(valueSat);
    if (sat === null) return "-";
    const sign = sat >= 0 ? "+" : "-";
    const abs = Math.abs(sat);
    if (abs >= 100_000_000) {
        return `${sign}${fmtBtc(abs / 100_000_000)} BTC`;
    }
    return `${sign}${fmtNum(abs)} sat`;
}

function fmtTime(ts = new Date()) {
    return ts.toLocaleString("zh-CN", { hour12: false });
}

function parseOptionalU32(text) {
    if (!text || text.trim() === "") return null;
    const n = Number(text);
    if (!Number.isInteger(n) || n < 0) {
        throw new Error("请输入非负整数高度");
    }
    return n;
}

function rpcErrorMessage(err) {
    if (typeof err === "string") return err;
    if (err?.message) return err.message;
    return JSON.stringify(err);
}

function isLikelyBitcoindRpcUrl(rawUrl) {
    try {
        const parsed = new URL(rawUrl);
        const host = parsed.hostname;
        const port = Number(parsed.port || (parsed.protocol === "https:" ? 443 : 80));
        const knownBitcoindPorts = new Set([8332, 18332, 18443, 38332, 48332, 28032, 28132]);
        return (
            (host === "127.0.0.1" || host === "localhost") &&
            knownBitcoindPorts.has(port)
        );
    } catch {
        return false;
    }
}

function normalizeNetworkName(network) {
    const raw = String(network || "").toLowerCase();
    if (raw.includes("regtest")) return "regtest";
    if (raw.includes("testnet4")) return "testnet4";
    if (raw.includes("testnet")) return "testnet";
    if (raw.includes("signet")) return "signet";
    if (raw.includes("mainnet") || raw.includes("bitcoin")) return "mainnet";
    return "";
}

async function rpcCall(method, params = []) {
    const body = {
        jsonrpc: "2.0",
        id: Date.now(),
        method,
        params,
    };

    const started = performance.now();
    let resp;
    try {
        resp = await fetch(state.rpcUrl, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(body),
        });
    } catch (err) {
        if (isLikelyBitcoindRpcUrl(state.rpcUrl)) {
            throw new Error(
                `当前地址看起来是 bitcoind RPC (${state.rpcUrl})，浏览器会被 CORS 拦截。请改为 usdb-indexer RPC（例如 http://127.0.0.1:28020 或 http://127.0.0.1:28120）。`,
            );
        }
        throw err;
    }
    const latency = Math.round(performance.now() - started);
    els.homeRpcLatency.textContent = `${latency} ms`;

    if (!resp.ok) {
        throw new Error(`HTTP ${resp.status}`);
    }

    const payload = await resp.json();
    if (payload.error) {
        throw new Error(rpcErrorMessage(payload.error));
    }
    return payload.result;
}

function setActiveTab(tabName) {
    els.tabs.forEach((el) => {
        el.classList.toggle("active", el.dataset.tab === tabName);
    });
    els.panels.forEach((el) => {
        el.classList.toggle("active", el.dataset.panel === tabName);
    });
}

function renderDetailGrid(container, entries) {
    container.innerHTML = "";
    entries.forEach(([k, v]) => {
        const row = document.createElement("div");
        row.className = "detail-item";
        row.innerHTML = `<span class="k">${k}</span><span class="v mono">${v ?? "-"}</span>`;
        container.appendChild(row);
    });
}

async function refreshHome() {
    els.homeError.textContent = "";
    try {
        const [rpcInfo, syncStatus, passStats, latestBalance] = await Promise.all([
            rpcCall("get_rpc_info"),
            rpcCall("get_sync_status"),
            rpcCall("get_pass_stats_at_height", [{ at_height: null }]),
            rpcCall("get_latest_active_balance_snapshot"),
        ]);

        els.homeNetwork.textContent = rpcInfo.network || "-";
        els.homeSyncedHeight.textContent = fmtNum(syncStatus.synced_block_height ?? 0);
        els.homeDependHeight.textContent = fmtNum(syncStatus.latest_depend_synced_block_height ?? 0);

        els.homeActivePass.textContent = fmtNum(passStats.active_count ?? 0);
        els.homeTotalPass.textContent = fmtNum(passStats.total_count ?? 0);
        els.homeActiveBalance.textContent = latestBalance
            ? fmtBalanceSmart(latestBalance.total_balance)
            : "-";

        const networkPreset = normalizeNetworkName(rpcInfo.network);
        if (networkPreset && els.rpcNetwork.value !== networkPreset) {
            els.rpcNetwork.value = networkPreset;
        }

        els.homeSyncMessage.textContent = syncStatus.message || "Running";
        els.homeSyncCurrent.textContent = fmtNum(syncStatus.current ?? 0);
        els.homeSyncTotal.textContent = fmtNum(syncStatus.total ?? 0);
        const total = Number(syncStatus.total || 0);
        const current = Number(syncStatus.current || 0);
        const pct = total > 0 ? Math.min(100, (current / total) * 100) : 0;
        els.homeSyncProgress.style.width = `${pct.toFixed(2)}%`;
        els.homeUpdatedAt.textContent = fmtTime();
        els.rpcHint.textContent = `连接正常，最后刷新 ${fmtTime()}`;
    } catch (err) {
        els.homeError.textContent = `首页刷新失败：${rpcErrorMessage(err)}`;
        els.rpcHint.textContent = `RPC 异常：${rpcErrorMessage(err)}`;
    }
}

function renderPassHistory(events) {
    els.passHistoryTable.innerHTML = "";
    events.forEach((event) => {
        const tr = document.createElement("tr");
        tr.innerHTML = `
            <td class="mono">${event.event_id}</td>
            <td>${fmtNum(event.block_height)}</td>
            <td>${event.event_type}</td>
            <td>${event.state}</td>
            <td class="mono">${event.owner}</td>
            <td class="mono">${event.satpoint}</td>
        `;
        els.passHistoryTable.appendChild(tr);
    });
}

async function loadPassHistory() {
    if (!state.pass.inscriptionId) return;
    els.passHistoryError.textContent = "";
    try {
        const page = await rpcCall("get_pass_history", [{
            inscription_id: state.pass.inscriptionId,
            from_height: state.pass.fromHeight,
            to_height: state.pass.toHeight,
            order: state.pass.order,
            page: state.pass.page,
            page_size: state.pass.pageSize,
        }]);

        state.pass.total = Number(page.total || 0);
        renderPassHistory(page.items || []);
        const currentPage = state.pass.page + 1;
        const totalPages = Math.max(1, Math.ceil(state.pass.total / state.pass.pageSize));
        els.passHistoryPage.textContent = `${currentPage}/${totalPages}`;
        els.passHistorySummary.textContent = `total=${fmtNum(state.pass.total)}, range=[${fmtNum(state.pass.fromHeight)}, ${fmtNum(state.pass.toHeight)}], order=${state.pass.order}`;
        els.passHistoryPrev.disabled = state.pass.page === 0;
        els.passHistoryNext.disabled = currentPage >= totalPages;
    } catch (err) {
        els.passHistoryError.textContent = `历史查询失败：${rpcErrorMessage(err)}`;
    }
}

async function queryPassSnapshot() {
    els.passQueryHint.textContent = "";
    els.passHistoryError.textContent = "";

    const inscriptionId = els.passIdInput.value.trim();
    if (!inscriptionId) {
        els.passQueryHint.textContent = "请输入 inscription id。";
        return;
    }

    try {
        state.pass.inscriptionId = inscriptionId;
        state.pass.atHeight = parseOptionalU32(els.passHeightInput.value);
        state.pass.page = 0;

        const snapshot = await rpcCall("get_pass_snapshot", [{
            inscription_id: inscriptionId,
            at_height: state.pass.atHeight,
        }]);

        if (!snapshot) {
            els.passSnapshotEmpty.textContent = "该矿工证不存在或在目标高度不可见。";
            els.passSnapshotEmpty.classList.remove("hidden");
            els.passSnapshotBox.classList.add("hidden");
            els.passHistoryTable.innerHTML = "";
            els.passHistorySummary.textContent = "-";
            return;
        }

        els.passSnapshotEmpty.classList.add("hidden");
        els.passSnapshotBox.classList.remove("hidden");
        renderDetailGrid(els.passSnapshotGrid, [
            ["inscription_id", snapshot.inscription_id],
            ["inscription_number", snapshot.inscription_number],
            ["resolved_height", snapshot.resolved_height],
            ["state", snapshot.state],
            ["owner", snapshot.owner],
            ["mint_block_height", snapshot.mint_block_height],
            ["mint_owner", snapshot.mint_owner],
            ["eth_main", snapshot.eth_main],
            ["eth_collab", snapshot.eth_collab || "-"],
            ["prev", (snapshot.prev || []).join(", ") || "-"],
            ["invalid_code", snapshot.invalid_code || "-"],
            ["invalid_reason", snapshot.invalid_reason || "-"],
            ["satpoint", snapshot.satpoint],
            ["last_event_id", snapshot.last_event_id],
            ["last_event_type", snapshot.last_event_type],
        ]);

        state.pass.fromHeight = Number(snapshot.mint_block_height || 0);
        state.pass.toHeight = Number(snapshot.resolved_height || 0);
        els.passQueryHint.textContent = "查询成功。";

        await loadPassHistory();
    } catch (err) {
        els.passQueryHint.textContent = `查询失败：${rpcErrorMessage(err)}`;
    }
}

function renderLeaderboardRows(rows) {
    els.energyLeaderboardTable.innerHTML = "";
    const rankBase = state.energy.leaderboard.page * state.energy.leaderboard.pageSize;
    rows.forEach((item, idx) => {
        const tr = document.createElement("tr");
        tr.className = "clickable";
        tr.innerHTML = `
            <td>${fmtNum(rankBase + idx + 1)}</td>
            <td>${fmtNum(item.energy)}</td>
            <td class="mono" title="${item.inscription_id}">${shortText(item.inscription_id, 14, 14)}</td>
            <td>${item.state}</td>
            <td>${fmtNum(item.record_block_height)}</td>
        `;
        tr.addEventListener("click", () => {
            els.energyIdInput.value = item.inscription_id;
            void queryEnergySnapshot();
        });
        els.energyLeaderboardTable.appendChild(tr);
    });
}

async function loadLeaderboard() {
    try {
        const page = await rpcCall("get_pass_energy_leaderboard", [{
            at_height: null,
            scope: state.energy.leaderboard.scope,
            page: state.energy.leaderboard.page,
            page_size: state.energy.leaderboard.pageSize,
        }]);

        state.energy.leaderboard.total = Number(page.total || 0);
        renderLeaderboardRows(page.items || []);

        const currentPage = state.energy.leaderboard.page + 1;
        const totalPages = Math.max(
            1,
            Math.ceil(state.energy.leaderboard.total / state.energy.leaderboard.pageSize),
        );
        els.energyLeaderboardPage.textContent = `${currentPage}/${totalPages}`;
        els.energyLeaderboardSummary.textContent = `resolved_height=${fmtNum(page.resolved_height)}, scope=${state.energy.leaderboard.scope}, total=${fmtNum(state.energy.leaderboard.total)}`;
        els.energyLeaderboardPrev.disabled = state.energy.leaderboard.page === 0;
        els.energyLeaderboardNext.disabled = currentPage >= totalPages;
    } catch (err) {
        els.energyLeaderboardSummary.textContent = `排行加载失败：${rpcErrorMessage(err)}`;
    }
}

function renderEnergyRangeRows(rows) {
    els.energyRangeTable.innerHTML = "";
    if (!rows || rows.length === 0) {
        const tr = document.createElement("tr");
        tr.innerHTML = `<td colspan="7">No energy records in selected range.</td>`;
        els.energyRangeTable.appendChild(tr);
        return;
    }
    rows.forEach((item) => {
        const deltaClass = item.owner_delta >= 0 ? "pos" : "neg";
        const tr = document.createElement("tr");
        tr.innerHTML = `
            <td>${fmtNum(item.record_block_height)}</td>
            <td>${item.state}</td>
            <td>${fmtNum(item.active_block_height)}</td>
            <td class="mono" title="${item.owner_address}">${shortText(item.owner_address, 14, 14)}</td>
            <td>${fmtBalanceSmart(item.owner_balance)}</td>
            <td class="${deltaClass}">${fmtBalanceDeltaSmart(item.owner_delta)}</td>
            <td>${fmtNum(item.energy)}</td>
        `;
        els.energyRangeTable.appendChild(tr);
    });
}

async function loadEnergyRange() {
    if (!state.energy.selectedInscriptionId) return;
    if (state.energy.range.fromHeight === null || state.energy.range.toHeight === null) return;
    els.energyRangeError.textContent = "";

    try {
        const page = await rpcCall("get_pass_energy_range", [{
            inscription_id: state.energy.selectedInscriptionId,
            from_height: state.energy.range.fromHeight,
            to_height: state.energy.range.toHeight,
            order: state.energy.range.order,
            page: state.energy.range.page,
            page_size: state.energy.range.pageSize,
        }]);

        state.energy.range.total = Number(page.total || 0);
        const rows = page.items || [];
        if (state.energy.range.total === 0 && state.energy.latestSnapshot) {
            // Defensive fallback: if timeline API unexpectedly returns empty but snapshot exists,
            // query exact latest record and render one-row timeline instead of blank table.
            const latest = state.energy.latestSnapshot;
            const exact = await rpcCall("get_pass_energy", [{
                inscription_id: latest.inscription_id,
                block_height: latest.record_block_height,
                mode: "exact",
            }]);
            renderEnergyRangeRows([{
                record_block_height: exact.record_block_height,
                state: exact.state,
                active_block_height: exact.active_block_height,
                owner_address: exact.owner_address,
                owner_balance: exact.owner_balance,
                owner_delta: exact.owner_delta,
                energy: exact.energy,
            }]);
            els.energyRangeSummary.textContent = `total=0 (fallback latest record shown), range=[${fmtNum(state.energy.range.fromHeight)}, ${fmtNum(state.energy.range.toHeight)}], order=${state.energy.range.order}`;
            els.energyRangePage.textContent = "1/1";
            els.energyRangePrev.disabled = true;
            els.energyRangeNext.disabled = true;
            return;
        }
        renderEnergyRangeRows(rows);

        const currentPage = state.energy.range.page + 1;
        const totalPages = Math.max(1, Math.ceil(state.energy.range.total / state.energy.range.pageSize));
        els.energyRangePage.textContent = `${currentPage}/${totalPages}`;
        els.energyRangeSummary.textContent = `total=${fmtNum(state.energy.range.total)}, range=[${fmtNum(state.energy.range.fromHeight)}, ${fmtNum(state.energy.range.toHeight)}], order=${state.energy.range.order}`;
        els.energyRangePrev.disabled = state.energy.range.page === 0;
        els.energyRangeNext.disabled = currentPage >= totalPages;
        return;
    } catch (err) {
        state.energy.range.total = 0;
        els.energyRangeError.textContent = `区间查询失败：${rpcErrorMessage(err)}`;
        throw err;
    }
}

async function queryEnergySnapshot() {
    const inscriptionId = els.energyIdInput.value.trim();
    if (!inscriptionId) {
        els.energyQueryHint.textContent = "请输入 inscription id。";
        return;
    }
    els.energyQueryHint.textContent = "";
    els.energyRangeError.textContent = "";

    try {
        const [snapshot, passSnapshot] = await Promise.all([
            rpcCall("get_pass_energy", [{
                inscription_id: inscriptionId,
                block_height: null,
                mode: "at_or_before",
            }]),
            rpcCall("get_pass_snapshot", [{
                inscription_id: inscriptionId,
                at_height: null,
            }]),
        ]);

        state.energy.selectedInscriptionId = inscriptionId;
        state.energy.latestSnapshot = snapshot;
        els.energySnapshotEmpty.classList.add("hidden");
        els.energySnapshotBox.classList.remove("hidden");
        renderDetailGrid(els.energySnapshotGrid, [
            ["inscription_id", snapshot.inscription_id],
            ["current_height", snapshot.query_block_height],
            ["current_state", snapshot.state],
            ["current_energy", fmtNum(snapshot.energy)],
            ["current_owner", snapshot.owner_address],
        ]);

        const toHeight = Number(snapshot.query_block_height || 0);
        const fromHeight = Math.max(0, Number(passSnapshot?.mint_block_height ?? 0));
        state.energy.range.fromHeight = fromHeight;
        state.energy.range.toHeight = toHeight;
        state.energy.range.page = 0;
        els.energyRangeFrom.value = String(fromHeight);
        els.energyRangeTo.value = String(toHeight);

        await loadEnergyRange();
        els.energyQueryHint.textContent = `查询成功。timeline_total=${fmtNum(state.energy.range.total)}`;
    } catch (err) {
        els.energyQueryHint.textContent = `查询失败：${rpcErrorMessage(err)}`;
    }
}

function bindEvents() {
    els.tabs.forEach((tab) => {
        tab.addEventListener("click", () => {
            setActiveTab(tab.dataset.tab);
        });
    });

    els.rpcForm.addEventListener("submit", (event) => {
        event.preventDefault();
        const url = els.rpcUrl.value.trim();
        if (!url) return;
        if (isLikelyBitcoindRpcUrl(url)) {
            els.rpcHint.textContent = "你输入的是 bitcoind RPC 端口，浏览器会触发 CORS。请使用 usdb-indexer RPC（默认 http://127.0.0.1:28020，regtest 常用 http://127.0.0.1:28120）。";
            return;
        }
        state.rpcUrl = url;
        els.rpcHint.textContent = `已切换 RPC: ${url}`;
        void refreshHome();
        void loadLeaderboard();
    });

    els.rpcNetwork.addEventListener("change", () => {
        const network = els.rpcNetwork.value;
        const defaultUrl = RPC_DEFAULT_ENDPOINTS[network];
        if (defaultUrl) {
            els.rpcUrl.value = defaultUrl;
            state.rpcUrl = defaultUrl;
            els.rpcHint.textContent = `已按网络预设填充 RPC: ${defaultUrl}`;
        }
    });

    els.homeRefresh.addEventListener("click", () => {
        void refreshHome();
    });

    els.passQueryForm.addEventListener("submit", (event) => {
        event.preventDefault();
        void queryPassSnapshot();
    });
    els.passHistoryPrev.addEventListener("click", () => {
        if (state.pass.page > 0) {
            state.pass.page -= 1;
            void loadPassHistory();
        }
    });
    els.passHistoryNext.addEventListener("click", () => {
        state.pass.page += 1;
        void loadPassHistory();
    });

    els.energyLeaderboardPrev.addEventListener("click", () => {
        if (state.energy.leaderboard.page > 0) {
            state.energy.leaderboard.page -= 1;
            void loadLeaderboard();
        }
    });
    els.energyLeaderboardNext.addEventListener("click", () => {
        state.energy.leaderboard.page += 1;
        void loadLeaderboard();
    });
    els.energyLeaderboardScope.addEventListener("change", () => {
        state.energy.leaderboard.scope = els.energyLeaderboardScope.value;
        state.energy.leaderboard.page = 0;
        void loadLeaderboard();
    });

    els.energyQueryForm.addEventListener("submit", (event) => {
        event.preventDefault();
        state.energy.range.page = 0;
        void queryEnergySnapshot();
    });

    els.energyRangeForm.addEventListener("submit", (event) => {
        event.preventDefault();
        try {
            state.energy.range.fromHeight = parseOptionalU32(els.energyRangeFrom.value);
            state.energy.range.toHeight = parseOptionalU32(els.energyRangeTo.value);
            if (state.energy.range.fromHeight === null || state.energy.range.toHeight === null) {
                throw new Error("请填写 from/to 高度");
            }
            if (state.energy.range.fromHeight > state.energy.range.toHeight) {
                throw new Error("from_height 不能大于 to_height");
            }
            state.energy.range.page = 0;
            void loadEnergyRange();
        } catch (err) {
            els.energyRangeError.textContent = rpcErrorMessage(err);
        }
    });

    els.energyRangePrev.addEventListener("click", () => {
        if (state.energy.range.page > 0) {
            state.energy.range.page -= 1;
            void loadEnergyRange();
        }
    });
    els.energyRangeNext.addEventListener("click", () => {
        state.energy.range.page += 1;
        void loadEnergyRange();
    });
}

function startClock() {
    if (state.clockTimer) clearInterval(state.clockTimer);
    const tick = () => {
        els.homeNow.textContent = fmtTime();
    };
    tick();
    state.clockTimer = setInterval(tick, 1000);
}

function startHomeRefresh() {
    if (state.homeTimer) clearInterval(state.homeTimer);
    state.homeTimer = setInterval(() => {
        void refreshHome();
    }, state.homeRefreshMs);
}

async function bootstrap() {
    if (els.energyLeaderboardScope && els.energyLeaderboardScope.value) {
        state.energy.leaderboard.scope = els.energyLeaderboardScope.value;
    }
    bindEvents();
    startClock();
    startHomeRefresh();
    await refreshHome();
    await loadLeaderboard();
}

void bootstrap();
