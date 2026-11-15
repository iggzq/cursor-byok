import { useEffect, useState } from "react";
import { api, type CursorHistoryStatus, type LocalAgentHistoryStatus, type OverviewMode, type OverviewSource, type OverviewSyncSettings, type OverviewSyncStatus } from "../../shared/api";
import { appStore } from "../../shared/store/appStore";
import { Button } from "../../shared/ui/Button";
import { ConfirmDialog } from "../../shared/ui/ConfirmDialog";
import { FormField, SecretTextInput, TextInput } from "../../shared/ui/FormControls";
import { Select } from "../../shared/ui/Select";
import { Switch } from "../../shared/ui/Switch";
import { TitledCard } from "../../shared/ui/TitledCard";
import { useMessage } from "../../shared/ui/message";
import styles from "./OverviewSyncSettingsCard.module.scss";

const DEFAULT_SYNC: OverviewSyncSettings = {
  mode: "local",
  center_url: "",
  api_key: "",
  device_id: "",
  device_name: "",
  include_cursor_history: false,
};

function formatTime(value: number | null) {
  if (!value) return t("尚未同步");
  return new Date(value).toLocaleString();
}

function normalizeUrl(value: string) {
  const trimmed = value.trim();
  if (!trimmed) return "";
  return /^[a-z]+:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`;
}

export function OverviewSyncSettingsCard() {
  const message = useMessage();
  const [saved, setSaved] = useState<OverviewSyncSettings | null>(null);
  const [draft, setDraft] = useState<OverviewSyncSettings>(DEFAULT_SYNC);
  const [status, setStatus] = useState<OverviewSyncStatus | null>(null);
  const [sources, setSources] = useState<OverviewSource[]>([]);
  const [history, setHistory] = useState<CursorHistoryStatus | null>(null);
  const [localAgentHistory, setLocalAgentHistory] = useState<LocalAgentHistoryStatus | null>(null);
  const [saving, setSaving] = useState(false);
  const [flushing, setFlushing] = useState(false);
  const [testing, setTesting] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [importingLocal, setImportingLocal] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);

  const loadStatus = async () => {
    const [nextStatus, nextSources, nextHistory, nextLocalAgentHistory] = await Promise.all([
      api.overviewSyncStatus(),
      api.overviewSources().catch(() => [] as OverviewSource[]),
      api.cursorHistoryStatus(),
      api.localAgentHistoryStatus(),
    ]);
    setStatus(nextStatus);
    setSources(nextSources);
    setHistory(nextHistory);
    setLocalAgentHistory(nextLocalAgentHistory);
  };

  useEffect(() => {
    void (async () => {
      const sync = await api.overviewSyncSettings();
      setSaved(sync);
      setDraft(sync);
      await loadStatus();
    })().catch((cause: unknown) => message(cause instanceof Error ? cause.message : String(cause)));
  }, [message]);

  const remote = draft.mode === "remote";
  const dirty = saved !== null && (
    saved.mode !== draft.mode
    || saved.center_url !== draft.center_url
    || saved.api_key !== draft.api_key
    || saved.device_name !== draft.device_name
    || saved.include_cursor_history !== draft.include_cursor_history
  );
  const historyEnabled = saved?.include_cursor_history === true;

  const save = async () => {
    try {
      setSaving(true);
      const next = await appStore.updateOverviewSync({
        ...draft,
        center_url: normalizeUrl(draft.center_url),
        device_name: draft.device_name.trim(),
      });
      setSaved(next);
      setDraft(next);
      await appStore.refresh();
      await loadStatus();
      message(t("概览数据设置已保存"));
    } catch (cause) {
      message(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  };

  const importHistory = async () => {
    try {
      setImporting(true);
      setHistory(await api.syncCursorHistory());
      await appStore.refresh();
      await loadStatus();
      message(t("Cursor 历史用量已导入"));
    } catch (cause) {
      message(cause instanceof Error ? cause.message : String(cause));
      await loadStatus().catch(() => undefined);
    } finally {
      setImporting(false);
    }
  };

  const importLocalAgentHistory = async () => {
    try {
      setImportingLocal(true);
      setLocalAgentHistory(await api.syncLocalAgentHistory());
      await appStore.refresh();
      await loadStatus();
      message(t("本地工具 Token 历史已重新扫描"));
    } catch (cause) {
      message(cause instanceof Error ? cause.message : String(cause));
      await loadStatus().catch(() => undefined);
    } finally {
      setImportingLocal(false);
    }
  };

  const localSourceCalls = (source: string) => localAgentHistory?.sources.find((item) => item.source === source)?.calls ?? 0;

  const flush = async () => {
    try {
      setFlushing(true);
      setStatus(await api.flushOverviewSync());
      await appStore.refresh();
      await loadStatus();
      message(t("已完成一次远程同步"));
    } catch (cause) {
      message(cause instanceof Error ? cause.message : String(cause));
      void loadStatus().catch(() => undefined);
    } finally {
      setFlushing(false);
    }
  };

  const test = async () => {
    try {
      setTesting(true);
      await loadStatus();
      if (status?.last_error) {
        message(status.last_error);
      } else {
        message(t("已连接汇总中心（{count} 台设备）", { count: sources.length }));
      }
    } catch (cause) {
      message(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setTesting(false);
    }
  };

  const reset = async () => {
    try {
      setResetting(true);
      setStatus(await api.resetOverviewSync());
      setConfirmReset(false);
      await loadStatus();
      message(t("同步进度已重置，将重新上传本机历史记录"));
    } catch (cause) {
      message(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setResetting(false);
    }
  };

  return <TitledCard title={t("概览数据")} action={<Button size="small" variant="primary" disabled={!dirty || saving} onClick={() => void save()}>
    {saving ? t("保存中…") : t("保存")}
  </Button>}>
    <div className={styles.content}>
      <div className={styles.row}>
        <div className={styles.description}>
          <strong>{t("数据来源")}</strong>
          <small>{t("本地模式只统计本机调用；远程汇总模式从独立部署的汇总中心读取各平台数据。")}</small>
        </div>
        <div className={styles.modeControl}>
          <Select
            value={draft.mode}
            ariaLabel={t("数据来源")}
            options={[
              { value: "local", label: t("本地模式") },
              { value: "remote", label: t("远程汇总模式") },
            ]}
            onChange={(value) => setDraft((current) => ({ ...current, mode: value as OverviewMode }))}
          />
        </div>
      </div>

      <div className={styles.row}>
        <div className={styles.description}>
          <strong>{t("包含 Cursor 历史")}</strong>
          <small>{t("把 Cursor 账号的用量（含 CmdK / Tab 和开始用 Cursor BYOK 之前的历史）也统计到概览里。数据来自本机已登录的 Cursor 账号，仅在手动导入时读取。")}</small>
        </div>
        <Switch
          checked={draft.include_cursor_history}
          label={t("包含 Cursor 历史")}
          disabled={saving}
          onChange={(checked) => setDraft((current) => ({ ...current, include_cursor_history: checked }))}
        />
      </div>

      {remote && <div className={styles.description}>
        <small>{t("远程汇总模式下 Cursor 历史与本地 AI 工具历史也会推送到汇总中心，并按稳定事件 ID 去重。")}</small>
      </div>}

      {historyEnabled && <div className={styles.status}>
        <div>
          <strong>{t("Cursor 历史用量")}</strong>
          <small>
            {history?.last_error
              ? history.last_error
              : t("已导入 {count} 条记录 · 最近导入 {time}", {
                count: history?.event_count ?? 0,
                time: formatTime(history?.last_sync_at_ms ?? null),
              })}
          </small>
        </div>
        <div className={styles.statusActions}>
          <Button size="small" disabled={importing || saving} onClick={() => void importHistory()}>
            {importing ? t("导入中…") : t("立即导入")}
          </Button>
        </div>
      </div>}

      <div className={styles.status}>
        <div>
          <strong>{t("本地 AI 工具历史")}</strong>
          <small>{t("自动读取本机 Codex、Claude Code 与 Claude CLI 的会话日志，包含历史记录；仅提取 Token 用量，不保存对话正文。")}</small>
          <small>
            {localAgentHistory?.last_error
              ? localAgentHistory.last_error
              : t("Codex {codex} 条 · Claude Code {claudeCode} 条 · Claude CLI {claudeCli} 条 · 最近扫描 {time}", {
                codex: localSourceCalls("codex"),
                claudeCode: localSourceCalls("claude_code"),
                claudeCli: localSourceCalls("claude_cli"),
                time: formatTime(localAgentHistory?.last_sync_at_ms ?? null),
              })}
          </small>
        </div>
        <div className={styles.statusActions}>
          <Button size="small" disabled={importingLocal} onClick={() => void importLocalAgentHistory()}>
            {importingLocal ? t("扫描中…") : t("重新扫描")}
          </Button>
        </div>
      </div>

      {remote && <>
        <FormField label={t("汇总中心地址")} hint={t("中心服务根地址，例如 http://10.0.0.5:4399，末尾不需要路径。")}>
          <TextInput
            value={draft.center_url}
            placeholder="http://10.0.0.5:4399"
            autoComplete="off"
            disabled={saving}
            onChange={(event) => setDraft((current) => ({ ...current, center_url: event.target.value }))}
          />
        </FormField>
        <FormField label={t("汇总密钥")} hint={t("必须与中心服务启动时配置的 CURSOR_OVERVIEW_COLLECTOR_API_KEY 一致。")}>
          <SecretTextInput
            value={draft.api_key}
            autoComplete="off"
            disabled={saving}
            onChange={(event) => setDraft((current) => ({ ...current, api_key: event.target.value }))}
          />
        </FormField>
        <FormField label={t("设备名称")} hint={t("用于在多台设备之间区分数据来源。")}>
          <TextInput
            value={draft.device_name}
            placeholder={t("例如 MacBook")}
            autoComplete="off"
            disabled={saving}
            onChange={(event) => setDraft((current) => ({ ...current, device_name: event.target.value }))}
          />
        </FormField>
        <div className={styles.status}>
          <div>
            <strong>{t("同步状态")}</strong>
            <small>
              {status
                ? t("待推送 {pending} 条 · 最近成功 {time}", { pending: status.pending_calls, time: formatTime(status.last_push_at_ms) })
                : t("读取中…")}
            </small>
            {sources.length > 0 && <small>{t("中心已收到 {count} 台设备的数据", { count: sources.length })}</small>}
            {status?.last_error && <small className={styles.error}>{status.last_error}</small>}
          </div>
          <div className={styles.statusActions}>
            <Button size="small" disabled={testing || saving || !saved} onClick={() => void test()}>
              {testing ? t("检测中…") : t("检测连接")}
            </Button>
            <Button size="small" disabled={flushing || saving || !saved || !remote} onClick={() => void flush()}>
              {flushing ? t("同步中…") : t("立即同步")}
            </Button>
          </div>
        </div>
        {remote && <div className={styles.resetRow}>
          <div>
            <strong>{t("重新上传历史")}</strong>
            <small>{t("清空同步进度后重传本机全部历史记录；中心按调用 ID 去重，不会产生重复数据。切换中心或中心数据被清空后使用。")}</small>
          </div>
          <Button size="small" disabled={resetting || saving || flushing || !saved} onClick={() => setConfirmReset(true)}>
            {resetting ? t("重置中…") : t("重新上传")}
          </Button>
        </div>}
        {sources.length > 0 && <div className={styles.sources}>
          {sources.map((source) => <div key={source.source_id} className={styles.source}>
            <span>{source.name}</span>
            <small>{t("{calls} 条 · 最近 {time}", { calls: source.call_count, time: formatTime(source.last_push_at_ms) })}</small>
          </div>)}
        </div>}
      </>}
    </div>
    <ConfirmDialog
      open={confirmReset}
      title={t("重新上传全部历史记录？")}
      busy={resetting}
      cancelLabel={t("取消")}
      confirmLabel={t("开始重新上传")}
      onCancel={() => setConfirmReset(false)}
      onConfirm={() => void reset()}
    >
      <div className={styles.confirmContent}>
        {t("将清空本机同步进度并重新上传全部历史调用记录。中心按调用 ID 去重，已有数据不会重复；数据量较大时上传需要一段时间。")}
      </div>
    </ConfirmDialog>
  </TitledCard>;
}
