const state = {
    rpcUrl: "http://127.0.0.1:8099",
    lastRows: [],
};

const elements = {
    rpcConfig: document.getElementById("rpc-config"),
    rpcUrl: document.getElementById("rpc-url"),
    rpcHint: document.getElementById("rpc-hint"),
    refreshStatus: document.getElementById("refresh-status"),
    metricNetwork: document.getElementById("metric-network"),
    metricHeight: document.getElementById("metric-height"),
    metricPhase: document.getElementById("metric-phase"),
    metricLatency: document.getElementById("metric-latency"),
    statusMessage: document.getElementById("status-message"),
    statusProgress: document.getElementById("status-progress"),
    statusCurrent: document.getElementById("status-current"),
    statusTotal: document.getElementById("status-total"),
    statusPhase: document.getElementById("status-phase"),
    singleQuery: document.getElementById("single-query"),
    singleScriptHash: document.getElementById("single-script-hash"),
    singleHeight: document.getElementById("single-height"),
    singleRangeStart: document.getElementById("single-range-start"),
    singleRangeEnd: document.getElementById("single-range-end"),
    singleQueryHint: document.getElementById("single-query-hint"),
    singleTable: document.getElementById("single-table"),
    singleSummary: document.getElementById("single-summary"),
    batchQuery: document.getElementById("batch-query"),
    batchScriptHashes: document.getElementById("batch-script-hashes"),
    batchHeight: document.getElementById("batch-height"),
    batchRangeStart: document.getElementById("batch-range-start"),
    batchRangeEnd: document.getElementById("batch-range-end"),
    batchSummary: document.getElementById("batch-summary"),
    batchTable: document.getElementById("batch-table"),
    balanceChart: document.getElementById("balance-chart"),
    deltaChart: document.getElementById("delta-chart"),
};

function formatNum(n) {
    return new Intl.NumberFormat("en-US").format(n);
}

function formatDelta(delta) {
    const value = Math.abs(delta);
    const sign = delta >= 0 ? "+" : "-";
    return `${sign}${formatNum(value)}`;
}

function rpcErrorMessage(err) {
    if (typeof err === "string") return err;
    if (err?.message) return err.message;
    return JSON.stringify(err);
}

async function rpcCall(method, params = []) {
    const payload = {
        jsonrpc: "2.0",
        method,
        params,
        id: Date.now(),
    };

    const startedAt = performance.now();
    const resp = await fetch(state.rpcUrl, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
    });
    const elapsed = Math.round(performance.now() - startedAt);
    elements.metricLatency.textContent = `${elapsed} ms`;

    if (!resp.ok) {
        throw new Error(`HTTP ${resp.status}`);
    }

    const data = await resp.json();
    if (data.error) {
        throw new Error(rpcErrorMessage(data.error));
    }

    return data.result;
}

async function refreshStatus() {
    try {
        const [network, height, status] = await Promise.all([
            rpcCall("get_network_type"),
            rpcCall("get_block_height"),
            rpcCall("get_sync_status"),
        ]);

        elements.metricNetwork.textContent = String(network);
        elements.metricHeight.textContent = formatNum(height);
        elements.metricPhase.textContent = status.phase;
        elements.statusMessage.textContent = status.message || "无状态消息";
        elements.statusCurrent.textContent = formatNum(status.current || 0);
        elements.statusTotal.textContent = formatNum(status.total || 0);
        elements.statusPhase.textContent = status.phase;

        const progress = status.total > 0 ? Math.min(100, (status.current / status.total) * 100) : 0;
        elements.statusProgress.style.width = `${progress.toFixed(2)}%`;

        elements.rpcHint.textContent = `连接正常，最后刷新：${new Date().toLocaleTimeString()}`;
    } catch (err) {
        elements.rpcHint.textContent = `连接失败：${rpcErrorMessage(err)}`;
        elements.rpcHint.classList.add("negative");
    }
}

function buildQueryMode(prefix) {
    const mode = document.querySelector(`input[name="${prefix}-mode"]:checked`).value;
    if (mode === "height") {
        const value = Number(document.getElementById(`${prefix}-height`).value);
        if (!Number.isFinite(value) || value < 0) {
            throw new Error("Height 模式下请填写有效的 height");
        }
        return { block_height: value, block_range: null };
    }

    if (mode === "range") {
        const start = Number(document.getElementById(`${prefix}-range-start`).value);
        const end = Number(document.getElementById(`${prefix}-range-end`).value);
        if (!Number.isFinite(start) || !Number.isFinite(end) || start < 0 || end < 0 || end <= start) {
            throw new Error("Range 模式下请填写合法区间，且 end > start");
        }
        return { block_height: null, block_range: { start, end } };
    }

    return { block_height: null, block_range: null };
}

function renderSingleRows(rows) {
    elements.singleTable.innerHTML = "";
    rows.forEach((row) => {
        const tr = document.createElement("tr");
        tr.innerHTML = `
            <td>${formatNum(row.block_height)}</td>
            <td class="${row.delta >= 0 ? "positive" : "negative"}">${formatDelta(row.delta)}</td>
            <td>${formatNum(row.balance)}</td>
        `;
        elements.singleTable.appendChild(tr);
    });
}

function renderSingleSummary(rows) {
    if (!rows.length) {
        elements.singleSummary.textContent = "无数据";
        return;
    }

    const latest = rows[rows.length - 1];
    const net = rows.reduce((acc, item) => acc + item.delta, 0);
    elements.singleSummary.textContent = `记录 ${rows.length} 条，最新高度 ${formatNum(latest.block_height)}，最新余额 ${formatNum(latest.balance)} sat，区间净变化 ${formatDelta(net)} sat`;
}

function drawLineChart(canvas, rows) {
    const ctx = canvas.getContext("2d");
    const w = canvas.width;
    const h = canvas.height;
    ctx.clearRect(0, 0, w, h);

    if (!rows.length) {
        ctx.fillStyle = "#6f8197";
        ctx.fillText("No data", 12, 20);
        return;
    }

    const values = rows.map((r) => r.balance);
    const min = Math.min(...values);
    const max = Math.max(...values);
    const pad = 24;

    ctx.strokeStyle = "#d7e2ef";
    ctx.beginPath();
    ctx.moveTo(pad, h - pad);
    ctx.lineTo(w - pad, h - pad);
    ctx.stroke();

    ctx.strokeStyle = "#007a6f";
    ctx.lineWidth = 2;
    ctx.beginPath();
    rows.forEach((row, i) => {
        const x = pad + (i / Math.max(rows.length - 1, 1)) * (w - pad * 2);
        const y = h - pad - ((row.balance - min) / Math.max(max - min, 1)) * (h - pad * 2);
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
    });
    ctx.stroke();
}

function drawDeltaChart(canvas, rows) {
    const ctx = canvas.getContext("2d");
    const w = canvas.width;
    const h = canvas.height;
    ctx.clearRect(0, 0, w, h);

    if (!rows.length) {
        ctx.fillStyle = "#6f8197";
        ctx.fillText("No data", 12, 20);
        return;
    }

    const maxAbs = Math.max(...rows.map((r) => Math.abs(r.delta)), 1);
    const pad = 24;
    const baseline = h / 2;
    const barW = Math.max(3, (w - pad * 2) / rows.length - 2);

    ctx.strokeStyle = "#d7e2ef";
    ctx.beginPath();
    ctx.moveTo(pad, baseline);
    ctx.lineTo(w - pad, baseline);
    ctx.stroke();

    rows.forEach((row, i) => {
        const x = pad + i * ((w - pad * 2) / rows.length);
        const height = (Math.abs(row.delta) / maxAbs) * (h / 2 - pad);
        const y = row.delta >= 0 ? baseline - height : baseline;
        ctx.fillStyle = row.delta >= 0 ? "#0a8d63" : "#b5383a";
        ctx.fillRect(x, y, barW, height);
    });
}

async function runSingleQuery(event) {
    event.preventDefault();
    try {
        const scriptHash = elements.singleScriptHash.value.trim();
        if (!scriptHash) throw new Error("请先输入 Script Hash");

        const mode = buildQueryMode("single");
        const rows = await rpcCall("get_address_balance", [{
            script_hash: scriptHash,
            block_height: mode.block_height,
            block_range: mode.block_range,
        }]);

        state.lastRows = Array.isArray(rows) ? rows : [];
        renderSingleRows(state.lastRows);
        renderSingleSummary(state.lastRows);
        drawLineChart(elements.balanceChart, state.lastRows);
        drawDeltaChart(elements.deltaChart, state.lastRows);
        elements.singleQueryHint.textContent = "查询成功";
    } catch (err) {
        elements.singleQueryHint.textContent = `查询失败：${rpcErrorMessage(err)}`;
        elements.singleQueryHint.classList.add("negative");
    }
}

function summarizeBatch(scriptHashes, rowsList) {
    const items = scriptHashes.map((hash, idx) => {
        const rows = rowsList[idx] || [];
        const net = rows.reduce((acc, item) => acc + item.delta, 0);
        const latest = rows[rows.length - 1] || { block_height: 0, balance: 0 };
        return {
            hash,
            records: rows.length,
            latestHeight: latest.block_height,
            latestBalance: latest.balance,
            net,
        };
    });

    return items;
}

function renderBatchTable(items) {
    elements.batchTable.innerHTML = "";
    items.forEach((item) => {
        const tr = document.createElement("tr");
        tr.innerHTML = `
            <td>${item.hash}</td>
            <td>${formatNum(item.records)}</td>
            <td>${formatNum(item.latestHeight)}</td>
            <td>${formatNum(item.latestBalance)}</td>
            <td class="${item.net >= 0 ? "positive" : "negative"}">${formatDelta(item.net)}</td>
        `;
        elements.batchTable.appendChild(tr);
    });
}

async function runBatchQuery(event) {
    event.preventDefault();
    try {
        const scriptHashes = elements.batchScriptHashes.value
            .split("\n")
            .map((it) => it.trim())
            .filter(Boolean);
        if (!scriptHashes.length) throw new Error("请至少输入一个 Script Hash");

        const mode = buildQueryMode("batch");
        const rowsList = await rpcCall("get_addresses_balances", [{
            script_hashes: scriptHashes,
            block_height: mode.block_height,
            block_range: mode.block_range,
        }]);

        const items = summarizeBatch(scriptHashes, rowsList);
        renderBatchTable(items);

        const totalLatest = items.reduce((acc, item) => acc + item.latestBalance, 0);
        const totalNet = items.reduce((acc, item) => acc + item.net, 0);
        elements.batchSummary.textContent = `共 ${items.length} 个地址，最新余额合计 ${formatNum(totalLatest)} sat，区间净变化 ${formatDelta(totalNet)} sat`;
    } catch (err) {
        elements.batchSummary.textContent = `批量查询失败：${rpcErrorMessage(err)}`;
        elements.batchSummary.classList.add("negative");
    }
}

function connectRpc(event) {
    event.preventDefault();
    const input = elements.rpcUrl.value.trim();
    if (!input) return;
    state.rpcUrl = input;
    elements.rpcHint.textContent = `已切换 RPC: ${state.rpcUrl}`;
    elements.rpcHint.classList.remove("negative");
    refreshStatus();
}

function bootstrap() {
    elements.rpcConfig.addEventListener("submit", connectRpc);
    elements.refreshStatus.addEventListener("click", refreshStatus);
    elements.singleQuery.addEventListener("submit", runSingleQuery);
    elements.batchQuery.addEventListener("submit", runBatchQuery);

    refreshStatus();
    setInterval(refreshStatus, 5000);
}

bootstrap();
