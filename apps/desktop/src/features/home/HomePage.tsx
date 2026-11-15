import { useEffect, useState } from "react";
import { api, pluginText, type Overview, type OverviewModelOption } from "../../shared/api";
import { ContributionCalendarChart } from "./charts/ContributionCalendarChart";
import { DailyTokenUsageChart } from "./charts/DailyTokenUsageChart";
import { HomeMetrics } from "./metrics/HomeMetrics";
import { PageContent } from "../../shell/layout/PageContent";
import type { VirtualPageSection } from "../../shell/layout/VirtualPage";
import { OverviewTimeRangeFilter, type OverviewRangePreset, type QuickPreset } from "./overview/OverviewTimeRangeFilter";
import { PageActions } from "../../shell/PageActions";
import { appStore, useAppStore } from "../../shared/store/appStore";
import { formatTimeInput, parseTimeInput } from "../../shared/utils/parseTimeInput";
import { modelProviderName } from "../../shared/utils/modelProvider";
import { claudeIcon, flatColorOrganizationIcon, openAiIcon } from "../../shared/ui/icons";
import { useI18n } from "../../i18n/store";

type TimeRange = { startMs: number; endMs: number };

const CALENDAR_DAYS = 365;
const DAY_MS = 24 * 60 * 60_000;

function contributionCalendarData(overview: Overview, endMs: number) {
  const tokensByDate = new Map<string, number>();
  const series = [
    ...overview.token_usage_series,
    ...(overview.cursor_history?.token_usage_series ?? []),
    ...(overview.local_agent_history?.token_usage_series ?? []),
  ];
  for (const bucket of series) {
    const date = new Date(bucket.bucket_start_ms).toISOString().slice(0, 10);
    const tokens = bucket.input_tokens + bucket.cache_read_tokens + bucket.cache_write_tokens + bucket.output_tokens;
    tokensByDate.set(date, (tokensByDate.get(date) ?? 0) + tokens);
  }
  const lastDay = new Date(Math.max(0, endMs - 1));
  lastDay.setUTCHours(0, 0, 0, 0);
  const firstDayMs = lastDay.getTime() - (CALENDAR_DAYS - 1) * DAY_MS;
  return Array.from({ length: CALENDAR_DAYS }, (_, offset) => {
    const date = new Date(firstDayMs + offset * DAY_MS).toISOString().slice(0, 10);
    return { date, tokens: tokensByDate.get(date) ?? 0 };
  });
}

function mergeTokenUsageSeries(overview: Overview) {
  const buckets = new Map<number, { inputTokens: number; cacheReadTokens: number; cacheWriteTokens: number; outputTokens: number }>();
  const add = (bucket: Overview["token_usage_series"][number]) => {
    const current = buckets.get(bucket.bucket_start_ms) ?? {
      inputTokens: 0,
      cacheReadTokens: 0,
      cacheWriteTokens: 0,
      outputTokens: 0,
    };
    current.inputTokens += bucket.input_tokens;
    current.cacheReadTokens += bucket.cache_read_tokens;
    current.cacheWriteTokens += bucket.cache_write_tokens;
    current.outputTokens += bucket.output_tokens;
    buckets.set(bucket.bucket_start_ms, current);
  };
  overview.token_usage_series.forEach(add);
  overview.cursor_history?.token_usage_series.forEach(add);
  overview.local_agent_history?.token_usage_series.forEach(add);
  return [...buckets.entries()]
    .sort(([left], [right]) => left - right)
    .map(([bucketStartMs, value]) => ({ bucketStartMs, ...value }));
}

function presetRange(preset: Exclude<OverviewRangePreset, "custom">, now = new Date()): TimeRange {
  const endMs = now.getTime();
  if (preset === "today") {
    const start = new Date(now);
    start.setHours(0, 0, 0, 0);
    return { startMs: start.getTime(), endMs };
  }
  if (preset === "month") {
    const start = new Date(now);
    start.setMonth(start.getMonth() - 1);
    return { startMs: start.getTime(), endMs };
  }
  const duration = preset === "ten-minutes" ? 10 * 60_000
    : preset === "hour" ? 60 * 60_000
    : 7 * 24 * 60 * 60_000;
  return { startMs: now.getTime() - duration, endMs };
}

export function HomePage() {
  const { overview, busy, models, plugins, overviewSync } = useAppStore();
  const { locale } = useI18n();
  const [preset, setPreset] = useState<OverviewRangePreset>("month");
  const [quick, setQuick] = useState<QuickPreset | null>(null);
  const [customRange, setCustomRange] = useState<TimeRange | null>(null);
  const [customOpen, setCustomOpen] = useState(false);
  const [customStart, setCustomStart] = useState("");
  const [customEnd, setCustomEnd] = useState("");
  const [selectedModels, setSelectedModels] = useState<string[]>([]);
  const [appliedModels, setAppliedModels] = useState<string[]>([]);
  const [rangeOverview, setRangeOverview] = useState<Overview | null>(null);
  const [rangeBusy, setRangeBusy] = useState(false);
  const [refreshVersion, setRefreshVersion] = useState(0);
  const [remoteModels, setRemoteModels] = useState<OverviewModelOption[]>([]);
  const selectedRange = preset === "custom" ? customRange : presetRange(preset);
  const remoteMode = overviewSync.mode === "remote";

  useEffect(() => {
    if (!remoteMode) {
      setRemoteModels([]);
      return;
    }
    let active = true;
    void api.overviewModels().then((next) => {
      if (active) setRemoteModels(next);
    }).catch(() => undefined);
    return () => { active = false; };
  }, [remoteMode, refreshVersion]);

  // Local hashes and collected keys are different namespaces, so a mode switch
  // must not carry a stale model filter across.
  useEffect(() => {
    setSelectedModels([]);
    setAppliedModels([]);
  }, [remoteMode]);

  useEffect(() => {
    if (!selectedRange) return;
    let active = true;
    setRangeBusy(true);
    void api.overview({
      ...selectedRange,
      modelHashes: appliedModels,
    }).then((next) => {
      if (active) setRangeOverview(next);
    }).finally(() => {
      if (active) setRangeBusy(false);
    });
    return () => { active = false; };
  }, [preset, customRange, overview, refreshVersion, appliedModels, remoteMode]);

  const filteredOverview = rangeOverview ?? overview;
  const dailyTokenUsage = mergeTokenUsageSeries(filteredOverview);
  const contribution = contributionCalendarData(filteredOverview, selectedRange?.endMs ?? Date.now());
  const metrics = {
    llmCalls: filteredOverview.metrics.llm_calls,
    successfulCalls: filteredOverview.metrics.successful_calls,
    failedCalls: filteredOverview.metrics.failed_calls,
    tokenUsage: filteredOverview.metrics.token_usage,
    promptTokens: filteredOverview.metrics.prompt_tokens,
    cacheReadTokens: filteredOverview.metrics.cache_read_tokens,
    cacheWriteTokens: filteredOverview.metrics.cache_write_tokens,
    deviceCount: filteredOverview.device_count ?? 0,
    remote: remoteMode,
    cursorHistory: filteredOverview.cursor_history
      ? {
        calls: filteredOverview.cursor_history.calls,
        inputTokens: filteredOverview.cursor_history.input_tokens,
        cacheReadTokens: filteredOverview.cursor_history.cache_read_tokens,
        cacheWriteTokens: filteredOverview.cursor_history.cache_write_tokens,
        outputTokens: filteredOverview.cursor_history.output_tokens,
        lastSyncAtMs: filteredOverview.cursor_history.last_sync_at_ms,
      }
      : null,
    localAgentHistory: filteredOverview.local_agent_history
      ? {
        calls: filteredOverview.local_agent_history.calls,
        inputTokens: filteredOverview.local_agent_history.input_tokens,
        cacheReadTokens: filteredOverview.local_agent_history.cache_read_tokens,
        cacheWriteTokens: filteredOverview.local_agent_history.cache_write_tokens,
        outputTokens: filteredOverview.local_agent_history.output_tokens,
        lastSyncAtMs: filteredOverview.local_agent_history.last_sync_at_ms,
        sources: filteredOverview.local_agent_history.sources,
      }
      : null,
  };
  const openCustom = (open: boolean) => {
    if (open && !customStart && !customEnd) {
      const now = new Date();
      setCustomEnd(formatTimeInput(now));
      setCustomStart(formatTimeInput(new Date(now.getTime() - 60 * 60_000)));
    }
    setCustomOpen(open);
  };
  const applyCustom = () => {
    const startMs = parseTimeInput(customStart);
    const endMs = parseTimeInput(customEnd);
    if (startMs === null || endMs === null || startMs >= endMs) return;
    setCustomRange({ startMs, endMs });
    setAppliedModels(selectedModels);
    setQuick(null);
    setPreset("custom");
    setCustomOpen(false);
  };
  const selectQuick = (durationMs: number) => {
    const end = new Date();
    const start = new Date(end.getTime() - durationMs);
    setCustomStart(formatTimeInput(start));
    setCustomEnd(formatTimeInput(end));
    setCustomRange({ startMs: start.getTime(), endMs: end.getTime() });
    setAppliedModels(selectedModels);
    setQuick(durationMs === 4 * 60 * 60_000 ? "four-hours" : "twenty-four-hours");
    setPreset("custom");
    setCustomOpen(false);
  };
  const selectPreset = (value: Exclude<OverviewRangePreset, "custom">) => {
    setPreset(value);
    setQuick(null);
    setCustomOpen(false);
  };
  const refresh = async () => {
    await appStore.refresh();
    setRefreshVersion((version) => version + 1);
  };
  const iconFor = (type: string) => type === "anthropic" ? claudeIcon : openAiIcon;
  const modelOptions = remoteMode
    ? remoteModels.map((model) => ({
      value: model.key,
      label: model.label,
      group: t("全部设备"),
      icon: flatColorOrganizationIcon,
    }))
    : [
      ...models.map((model) => ({
        value: model.model_hash,
        label: model.display_name,
        group: modelProviderName(model),
        icon: iconFor(model.type),
      })),
      ...plugins.flatMap((plugin) => plugin.providers.flatMap((provider) =>
        provider.configured ? provider.models.filter((model) => model.enabled).map((model) => ({
          value: model.id,
          label: model.displayName,
          group: pluginText(provider.displayName, locale) || model.pluginName,
          iconSrc: model.icon || undefined,
          icon: model.icon ? undefined : flatColorOrganizationIcon,
        })) : [],
      )),
    ];
  const sections: VirtualPageSection[] = [
    {
      key: "daily-token-usage",
      estimatedHeight: 240,
      content: <DailyTokenUsageChart
        data={dailyTokenUsage}
        granularity={filteredOverview.token_usage_granularity}
      />,
    },
    {
      key: "metrics",
      estimatedHeight: 130,
      content: <HomeMetrics data={metrics} refreshVersion={refreshVersion} />,
    },

    {
      key: "activity",
      estimatedHeight: 106,
      content: <ContributionCalendarChart data={contribution} />,
    },
  ];

  return <>
    <PageActions><OverviewTimeRangeFilter
      value={preset}
      quick={quick}
      customOpen={customOpen}
      customStart={customStart}
      customEnd={customEnd}
      modelOptions={modelOptions}
      selectedModels={selectedModels}
      busy={busy || rangeBusy}
      onSelect={selectPreset}
      onQuickSelect={selectQuick}
      onCustomOpenChange={openCustom}
      onCustomStartChange={setCustomStart}
      onCustomEndChange={setCustomEnd}
      onSelectedModelsChange={setSelectedModels}
      onCustomApply={applyCustom}
      onRefresh={() => void refresh()}
    /></PageActions>
    <PageContent title={t("概览")} sections={sections} />
  </>;
}
