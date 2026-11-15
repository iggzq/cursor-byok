import { formatCompactInteger, formatInteger } from "../../../shared/utils/numberFormat";
import { useAppStore } from "../../../shared/store/appStore";
import type { LocalAgentUsageSource } from "../../../shared/api";
import { Icon } from "../../../shared/ui/Icon";
import { useTooltip, type TooltipAnchor } from "../../../shared/ui/Tooltip";
import { informationOutlineIcon } from "../../../shared/ui/icons";
import { CacheHitRateChart } from "./CacheHitRateChart";
import styles from "./HomeMetrics.module.scss";

export type HomeMetricsData = {
  llmCalls: number;
  successfulCalls: number;
  failedCalls: number;
  tokenUsage: number;
  promptTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  deviceCount?: number;
  remote?: boolean;
  cursorHistory?: CursorHistoryData | null;
  localAgentHistory?: LocalAgentHistoryData | null;
};

type HistoryData = {
  calls: number;
  inputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  outputTokens: number;
  lastSyncAtMs: number | null;
};

export type CursorHistoryData = HistoryData;

export type LocalAgentHistoryData = HistoryData & {
  sources: LocalAgentUsageSource[];
};

function formatMetricValue(value: number) {
  const full = formatInteger(value);
  const compact = formatCompactInteger(value);
  return full === compact ? full : `${full} (${compact})`;
}

function formatRate(value: number | null) {
  return value === null ? t("暂无数据") : `${(Math.max(0, Math.min(1, value)) * 100).toFixed(2)}%`;
}

function calculateRate(numerator: number, denominator: number) {
  return denominator > 0 ? numerator / denominator : null;
}

function priceTokens(tokens: number, pricePerMillion: number) {
  return (tokens / 1_000_000) * pricePerMillion;
}

function formatCNY(value: number) {
  return `¥${value.toFixed(2)}`;
}

function elementAnchor(element: HTMLElement): TooltipAnchor {
  return {
    contextElement: element,
    getBoundingClientRect: () => element.getBoundingClientRect(),
  };
}

function InfoTooltip({ content }: { content: string }) {
  const { show, hide } = useTooltip();

  return <button
    type="button"
    className={styles.info}
    aria-label={t("查看说明")}
    onMouseEnter={(event) => show(elementAnchor(event.currentTarget), undefined, <div className={styles.tooltipText}>{content}</div>)}
    onMouseLeave={hide}
    onFocus={(event) => show(elementAnchor(event.currentTarget), undefined, <div className={styles.tooltipText}>{content}</div>)}
    onBlur={hide}
  ><Icon icon={informationOutlineIcon} size="1.1em" /></button>;
}

export function HomeMetrics({ data, refreshVersion = 0 }: { data: HomeMetricsData; refreshVersion?: number }) {
  const { pricing } = useAppStore();
  const cursorHistory = data.cursorHistory ?? null;
  const localAgentHistory = data.localAgentHistory ?? null;
  const historyInputTokens = (cursorHistory?.inputTokens ?? 0) + (localAgentHistory?.inputTokens ?? 0);
  const historyCacheReadTokens = (cursorHistory?.cacheReadTokens ?? 0) + (localAgentHistory?.cacheReadTokens ?? 0);
  const historyCacheWriteTokens = (cursorHistory?.cacheWriteTokens ?? 0) + (localAgentHistory?.cacheWriteTokens ?? 0);
  const historyOutputTokens = (cursorHistory?.outputTokens ?? 0) + (localAgentHistory?.outputTokens ?? 0);
  const historyPromptTokens = historyInputTokens + historyCacheReadTokens + historyCacheWriteTokens;
  const historyTokenUsage = historyPromptTokens + historyOutputTokens;
  const cursorHistoryCalls = cursorHistory?.calls ?? 0;
  const localAgentHistoryCalls = localAgentHistory?.calls ?? 0;
  const historyCalls = cursorHistoryCalls + localAgentHistoryCalls;
  const includesCursorHistory = cursorHistory !== null && cursorHistory.calls > 0;
  const includesLocalAgentHistory = localAgentHistory !== null && localAgentHistory.calls > 0;
  const includesHistory = historyCalls > 0;
  const llmCalls = data.llmCalls + historyCalls;
  const tokenUsage = data.tokenUsage + historyTokenUsage;
  const promptTokens = data.promptTokens + historyPromptTokens;
  const cacheReadTokens = data.cacheReadTokens + historyCacheReadTokens;
  const cacheWriteTokens = data.cacheWriteTokens + historyCacheWriteTokens;
  const inputTokens = Math.max(0, data.promptTokens - data.cacheReadTokens - data.cacheWriteTokens);
  const outputTokens = Math.max(0, data.tokenUsage - data.promptTokens);
  const defaultCacheHitRate = calculateRate(cacheReadTokens, cacheReadTokens + inputTokens + historyInputTokens);
  const cacheReuseRate = calculateRate(
    cacheReadTokens,
    cacheReadTokens + cacheWriteTokens + inputTokens + historyInputTokens,
  );
  const successfulCallRate = calculateRate(data.successfulCalls, data.llmCalls);
  const costs = {
    input: priceTokens(inputTokens + historyInputTokens, pricing.input_per_million),
    output: priceTokens(outputTokens + historyOutputTokens, pricing.output_per_million),
    cacheRead: priceTokens(cacheReadTokens, pricing.cache_read_per_million),
    cacheWrite: priceTokens(cacheWriteTokens, pricing.cache_write_per_million),
  };
  const totalCost = costs.input + costs.output + costs.cacheRead + costs.cacheWrite;
  const cacheCost = costs.cacheRead + costs.cacheWrite;
  const cacheTooltip = [
    t("当前：{rate}", { rate: formatRate(defaultCacheHitRate) }),
    t("公式：缓存读取 /（缓存读取 + 非缓存输入）"),
    t("默认 {defaultRate} / 计入创建 {reuseRate}", {
      defaultRate: formatRate(defaultCacheHitRate),
      reuseRate: formatRate(cacheReuseRate),
    }),
  ].join("\n");
  const localHistorySourceLines = (localAgentHistory?.sources ?? [])
    .filter((source) => source.calls > 0)
    .map((source) => {
      const label = source.source === "codex" ? "Codex"
        : source.source === "claude_code" ? "Claude Code"
        : source.source === "claude_cli" ? "Claude CLI"
        : source.source;
      const tokens = source.input_tokens + source.cache_read_tokens + source.cache_write_tokens + source.output_tokens;
      return t("{source}：{count} 次，{tokens} Token", {
        source: label,
        count: formatMetricValue(source.calls),
        tokens: formatMetricValue(tokens),
      });
    });
  const callsTooltip = [
    data.remote ? t("按汇总中心收集的调用记录汇总，进行中的调用不计入。") : t("按历史 LLM 调用记录汇总，进行中的调用不计入。"),
    data.remote && data.deviceCount ? t("参与汇总的设备：{count} 台", { count: formatInteger(data.deviceCount) }) : "",
    "",
    t("总调用：{count}", { count: formatMetricValue(llmCalls) }),
    t("成功调用：{count}", { count: formatMetricValue(data.successfulCalls) }),
    t("异常调用：{count}", { count: formatMetricValue(data.failedCalls) }),
    t("成功占比：{rate}", { rate: formatRate(successfulCallRate) }),
    includesCursorHistory ? t("其中 Cursor 历史：{count}", { count: formatMetricValue(cursorHistoryCalls) }) : "",
    includesLocalAgentHistory ? t("其中本地 AI 工具历史：{count}", { count: formatMetricValue(localAgentHistoryCalls) }) : "",
    ...localHistorySourceLines,
  ].join("\n");
  const tokensTooltip = [
    t("总请求 Token 包含提示词和模型输出。"),
    "",
    t("总请求：{tokens}", { tokens: formatMetricValue(tokenUsage) }),
    t("提示词：{tokens}", { tokens: formatMetricValue(promptTokens) }),
    t("输出推算：{tokens}", { tokens: formatMetricValue(outputTokens + historyOutputTokens) }),
    t("非缓存输入：{tokens}", { tokens: formatMetricValue(inputTokens + historyInputTokens) }),
    t("缓存读取：{tokens}", { tokens: formatMetricValue(cacheReadTokens) }),
    t("缓存写入：{tokens}", { tokens: formatMetricValue(cacheWriteTokens) }),
    includesCursorHistory ? t("Cursor 历史：{tokens}", { tokens: formatMetricValue((cursorHistory?.inputTokens ?? 0) + (cursorHistory?.cacheReadTokens ?? 0) + (cursorHistory?.cacheWriteTokens ?? 0) + (cursorHistory?.outputTokens ?? 0)) }) : "",
    includesLocalAgentHistory ? t("本地 AI 工具历史：{tokens}", { tokens: formatMetricValue((localAgentHistory?.inputTokens ?? 0) + (localAgentHistory?.cacheReadTokens ?? 0) + (localAgentHistory?.cacheWriteTokens ?? 0) + (localAgentHistory?.outputTokens ?? 0)) }) : "",
    "",
    t("缓存读写已计入提示词侧统计。"),
  ].join("\n");
  const costTooltip = [
    t("按配置的 Token 价格估算。"),
    t("缓存统计策略：默认口径（{rate}）", { rate: formatRate(defaultCacheHitRate) }),
    "",
    t("普通输入：{tokens} × ¥{price}/Token = {cost}", {
      tokens: formatMetricValue(inputTokens + historyInputTokens),
      price: pricing.input_per_million / 1_000_000,
      cost: formatCNY(costs.input),
    }),
    t("模型输出：{tokens} × ¥{price}/Token = {cost}", {
      tokens: formatMetricValue(outputTokens + historyOutputTokens),
      price: pricing.output_per_million / 1_000_000,
      cost: formatCNY(costs.output),
    }),
    t("缓存读取：{tokens} × ¥{price}/Token = {cost}", {
      tokens: formatMetricValue(cacheReadTokens),
      price: pricing.cache_read_per_million / 1_000_000,
      cost: formatCNY(costs.cacheRead),
    }),
    t("缓存写入：{tokens} × ¥{price}/Token = {cost}", {
      tokens: formatMetricValue(cacheWriteTokens),
      price: pricing.cache_write_per_million / 1_000_000,
      cost: formatCNY(costs.cacheWrite),
    }),
    "",
    t("合计：{cost}", { cost: formatCNY(totalCost) }),
  ].join("\n");

  return <div className={styles.scroller}>
    <section className={styles.root} aria-label={t("调用统计")}>
      <article className={styles.metric}>
        <div className={styles.label}>{t("缓存命中率")}<InfoTooltip content={cacheTooltip} /></div>
        <CacheHitRateChart rate={defaultCacheHitRate ?? 0} animationKey={refreshVersion} />
      </article>
      <article className={styles.metric}>
        <div className={styles.label}>{t("LLM 调用")}<InfoTooltip content={callsTooltip} /></div>
        <div className={styles.body}>
          <div className={styles.value} title={formatInteger(llmCalls)}>{formatCompactInteger(llmCalls)}</div>
          <div className={styles.secondary}>{t("成功 {successful} / 异常 {failed}", {
            successful: formatCompactInteger(data.successfulCalls),
            failed: formatCompactInteger(data.failedCalls),
          })}{includesCursorHistory ? t(" · Cursor 历史 {count}", { count: formatCompactInteger(cursorHistoryCalls) }) : ""}{includesLocalAgentHistory ? t(" · 本地工具 {count}", { count: formatCompactInteger(localAgentHistoryCalls) }) : ""}{data.remote && data.deviceCount ? t(" · {count} 台设备", { count: formatCompactInteger(data.deviceCount) }) : ""}</div>
        </div>
      </article>
      <article className={styles.metric}>
        <div className={styles.label}>{t("Token 消耗")}<InfoTooltip content={tokensTooltip} /></div>
        <div className={styles.body}>
          <div className={styles.value} title={formatInteger(tokenUsage)}>{formatCompactInteger(tokenUsage)}</div>
          <div className={styles.secondary}>{t("提示词 {tokens}", { tokens: formatCompactInteger(promptTokens) })}{includesHistory ? t(" · 历史 {tokens}", { tokens: formatCompactInteger(historyTokenUsage) }) : ""}</div>
        </div>
      </article>
      <article className={styles.metric}>
        <div className={styles.label}>{t("价值估算")}<InfoTooltip content={costTooltip} /></div>
        <div className={styles.body}>
          <div className={styles.value} title={formatCNY(totalCost)}>{formatCNY(totalCost)}</div>
          <div className={styles.secondary}>{t("缓存读写 {cost}", { cost: formatCNY(cacheCost) })}</div>
        </div>
      </article>
    </section>
  </div>;
}
