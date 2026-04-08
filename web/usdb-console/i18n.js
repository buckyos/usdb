(function initUsdbConsoleI18n(global) {
    const translations = {
        en: {
            "locales.en": "English",
            "locales.zh-CN": "简体中文",
            "actions.language": "Language",
            "actions.refresh": "Refresh",
            "hero.subtitle": "View BTC, USDB, and ETHW service health, bootstrap artifacts, and operational entry points in one place.",
            "hero.hint": "The first release focuses on a read-only overview. Wallet connections and write actions will come later.",
            "metrics.updatedAt": "Updated",
            "metrics.btcNetwork": "BTC Network",
            "metrics.btcHeight": "BTC Block Height",
            "metrics.ethwHeight": "ETHW Block Height",
            "sections.services": "Service Status",
            "sections.bootstrapManifest": "Bootstrap Manifest",
            "sections.snapshotMarker": "Snapshot Marker",
            "sections.ethwInitMarker": "ETHW Init Marker",
            "sections.coldStartSteps": "Cold-Start Steps",
            "sections.explorers": "Explorer Entry Points",
            "explorers.hint": "The existing pages remain as standalone service explorers. The console links to them from a single overview.",
            "explorers.balanceHistoryDescription": "Inspect address balances, sync progress, and UTXO details.",
            "explorers.usdbIndexerDescription": "Inspect miner passes, energy, rankings, and protocol state.",
            "services.summary": "Showing {readyCount}/{total} core services reachable. The overview focuses on readiness, cold-start steps, and explorer entry points.",
            "serviceStates.offline": "offline",
            "serviceStates.reachable": "reachable",
            "serviceStates.queryReady": "query ready",
            "serviceStates.consensusReady": "consensus ready",
            "artifactStates.missing": "missing",
            "artifactStates.present": "present",
            "states.pending": "pending",
            "states.in_progress": "in progress",
            "states.completed": "completed",
            "states.error": "error",
            "bootstrap.steps.snapshot-loader": "Snapshot Loader",
            "bootstrap.steps.bootstrap-init": "Bootstrap Init",
            "bootstrap.steps.ethw-init": "ETHW Init",
            "bootstrap.steps.sourcedao-bootstrap": "SourceDAO Bootstrap",
            "bootstrap.stepDetails.pending": "{label}: waiting for artifact at {path}",
            "bootstrap.stepDetails.completed": "{label}: using {path}",
            "bootstrap.stepDetails.error": "{label}: {error}",
            "fields.chain": "Chain",
            "fields.blocks": "Blocks",
            "fields.headers": "Headers",
            "fields.ibd": "IBD",
            "fields.verifyProgress": "Verify Progress",
            "fields.latency": "Latency",
            "fields.network": "Network",
            "fields.stableHeight": "Stable Height",
            "fields.phase": "Phase",
            "fields.consensus": "Consensus",
            "fields.snapshotVerify": "Snapshot Verify",
            "fields.blockers": "Blockers",
            "fields.syncedHeight": "Synced Height",
            "fields.systemState": "System State",
            "fields.client": "Client",
            "fields.chainId": "Chain ID",
            "fields.networkId": "Network ID",
            "fields.blockNumber": "Block Number",
            "fields.syncing": "Syncing",
            "common.none": "-",
            "common.false": "false",
        },
        "zh-CN": {
            "locales.en": "English",
            "locales.zh-CN": "简体中文",
            "actions.language": "语言",
            "actions.refresh": "刷新状态",
            "hero.subtitle": "统一查看 BTC、USDB 与 ETHW 本地服务状态、bootstrap 产物和运维入口。",
            "hero.hint": "第一版优先提供只读总览，钱包接入与写操作后续再加入。",
            "metrics.updatedAt": "更新时间",
            "metrics.btcNetwork": "BTC 网络",
            "metrics.btcHeight": "BTC 区块高度",
            "metrics.ethwHeight": "ETHW 区块高度",
            "sections.services": "服务状态",
            "sections.bootstrapManifest": "Bootstrap 清单",
            "sections.snapshotMarker": "快照标记",
            "sections.ethwInitMarker": "ETHW 初始化标记",
            "sections.coldStartSteps": "冷启动步骤",
            "sections.explorers": "Explorer 入口",
            "explorers.hint": "现有页面继续保留为独立的服务 Explorer，由控制台统一跳转。",
            "explorers.balanceHistoryDescription": "查看地址余额、同步进度与 UTXO 细节。",
            "explorers.usdbIndexerDescription": "查看矿工证、能量、排行和协议状态。",
            "services.summary": "当前 {readyCount}/{total} 个核心服务可达；首页优先展示 readiness、冷启动步骤和 Explorer 入口。",
            "serviceStates.offline": "离线",
            "serviceStates.reachable": "可达",
            "serviceStates.queryReady": "查询就绪",
            "serviceStates.consensusReady": "共识就绪",
            "artifactStates.missing": "缺失",
            "artifactStates.present": "已就绪",
            "states.pending": "待处理",
            "states.in_progress": "进行中",
            "states.completed": "已完成",
            "states.error": "错误",
            "bootstrap.steps.snapshot-loader": "快照安装",
            "bootstrap.steps.bootstrap-init": "Bootstrap 初始化",
            "bootstrap.steps.ethw-init": "ETHW 初始化",
            "bootstrap.steps.sourcedao-bootstrap": "SourceDAO 初始化",
            "bootstrap.stepDetails.pending": "{label}: 等待产物 {path}",
            "bootstrap.stepDetails.completed": "{label}: 使用 {path}",
            "bootstrap.stepDetails.error": "{label}: {error}",
            "fields.chain": "链",
            "fields.blocks": "区块",
            "fields.headers": "头部",
            "fields.ibd": "IBD",
            "fields.verifyProgress": "校验进度",
            "fields.latency": "延迟",
            "fields.network": "网络",
            "fields.stableHeight": "稳定高度",
            "fields.phase": "阶段",
            "fields.consensus": "共识状态",
            "fields.snapshotVerify": "快照校验",
            "fields.blockers": "阻塞项",
            "fields.syncedHeight": "同步高度",
            "fields.systemState": "系统状态",
            "fields.client": "客户端",
            "fields.chainId": "链 ID",
            "fields.networkId": "网络 ID",
            "fields.blockNumber": "区块高度",
            "fields.syncing": "同步状态",
            "common.none": "-",
            "common.false": "false",
        },
    };

    function normalizeLocale(locale) {
        if (!locale) return "en";
        if (translations[locale]) return locale;
        const lower = String(locale).toLowerCase();
        if (lower.startsWith("zh")) return "zh-CN";
        return "en";
    }

    function resolveLocale() {
        const params = new URLSearchParams(global.location.search);
        const urlLocale = normalizeLocale(params.get("lang"));
        if (params.has("lang")) return urlLocale;

        const storedLocale = normalizeLocale(global.localStorage.getItem("usdb-console.locale"));
        if (global.localStorage.getItem("usdb-console.locale")) return storedLocale;

        return normalizeLocale(global.navigator.language || "en");
    }

    function interpolate(template, params = {}) {
        return template.replace(/\{([^}]+)\}/g, (_, key) => {
            const value = params[key];
            return value === undefined || value === null ? "" : String(value);
        });
    }

    function t(locale, key, params = {}) {
        const normalized = normalizeLocale(locale);
        const value = translations[normalized][key] ?? translations.en[key] ?? key;
        return interpolate(value, params);
    }

    global.USDBConsoleI18n = {
        translations,
        normalizeLocale,
        resolveLocale,
        t,
    };
})(window);
