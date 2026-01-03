import { useState, useEffect, useMemo } from 'react';
import axios from 'axios';
import { Panel, Group as PanelGroup, Separator as PanelResizeHandle } from 'react-resizable-panels';
import { 
  Terminal, 
  Clock, 
  Database,
  Search,
  ChevronDown,
  Wrench,
  ExternalLink,
  GitCompare,
  Tag
} from 'lucide-react';

const API_BASE = window.location.origin + '/debug';

// Parse XML-like tags from text content
interface ParsedTag {
  name: string;
  content: string;
  attributes?: Record<string, string>;
}

// Tag delta from backend
interface TagDelta {
  tag: string;
  content: string;
  status: 'new' | 'modified' | 'unchanged' | 'removed';
  previous_content?: string;
}

// Format tool arguments for display
function formatArgsPreview(argsSource: string | undefined): string {
  if (!argsSource) return 'N/A';
  
  try {
    const args = JSON.parse(argsSource);
    if (typeof args !== 'object' || args === null) {
      return String(args);
    }
    
    // Format as "key1: value1, key2: value2, ..."
    const pairs = Object.entries(args)
      .map(([key, value]) => {
        let displayValue: string;
        if (typeof value === 'string') {
          displayValue = value.length > 30 ? value.slice(0, 30) + '...' : value;
        } else if (typeof value === 'object') {
          displayValue = JSON.stringify(value).slice(0, 30) + '...';
        } else {
          displayValue = String(value);
        }
        return `${key}: ${displayValue}`;
      })
      .join(', ');
    
    return pairs.length > 60 ? pairs.slice(0, 60) + '...' : pairs;
  } catch {
    // If parsing fails, return as-is
    return argsSource.length > 60 ? argsSource.slice(0, 60) + '...' : argsSource;
  }
}

interface Issue {
  kind: string;
  severity: string;
  message: string;
  context: unknown;
}

interface IssueCounts {
  tool_args_empty: number;
  tool_args_repaired: number;
  rescue_used: number;
  tag_unregistered: number;
  tag_leak_echo: number;
  reasoning_leak: number;
  tool_args_invalid: number;
  tool_call_duplicate_id: number;
}

interface StageDiffResponse {
  diff: unknown;
}

interface BlobRef {
  blob_id: string;
  content_type: string;
  approx_bytes: number;
  file_name?: string;
  sha256?: string;
  written_at_ms?: number;
}

interface Stage {
  name: string;
  kind: string;
  summary: unknown;
  blob_ref?: BlobRef;
}

interface ToolCallIndex {
  id: string;
  name: string;
  args_status: string;
  origin: string;
  evidence: {
    stage: string;
    message_index?: number;
    snippet?: string;
    blob_ref?: BlobRef;
    offsets?: { start: number; end: number };
    request_tool_index?: number;
    response_tool_index?: number;
    raw_arguments_snippet?: string;
  };
}

interface ToolResultIndex {
  tool_call_id: string;
  name?: string;
  snippet: string;
  is_error: boolean;
  blob_ref?: BlobRef;
}

interface TagLocation {
  stage: string;
  message_index?: number;
  offsets?: { start: number; end: number };
  snippet: string;
  blob_ref?: BlobRef;
}

interface TagOccurrence {
  tag: string;
  count: number;
  locations: TagLocation[];
}

interface TagSummary {
  registered: TagOccurrence[];
  unregistered: TagOccurrence[];
  leaks: TagOccurrence[];
}

interface TurnSummary {
  turn_id: string;
  request_id: string;
  model_id: string;
  flavor: string;
  started_at_ms: number;
  ended_at_ms: number | null;
  issues: IssueCounts;
  role?: string;
}

interface Conversation {
  conversation_id: string;
  created_at_ms: number;
  last_updated_ms: number;
  turns: TurnSummary[];
  issues: IssueCounts;
}

interface TurnDetail {
  turn_id: string;
  request_id: string;
  model_id: string;
  flavor: string;
  started_at_ms: number;
  ended_at_ms: number | null;
  stages: Stage[];
  tool_calls: ToolCallIndex[];
  cursor_tags: TagSummary;
  issues: Issue[];
  trace_id?: string;
  span_summary?: Array<{
    name: string;
    level: string;
    fields: unknown;
  }>;
  user_query?: string;
  user_query_tags?: TagDelta[];
  tool_results: ToolResultIndex[];
  role?: string;
}

function TagsDisplay({ tags, deltas }: { tags?: ParsedTag[]; deltas?: TagDelta[] }) {
  const [expandedTags, setExpandedTags] = useState<Set<string>>(new Set());
  const [viewMode, setViewMode] = useState<'all' | 'changes-only'>('all');

  const toggleTag = (tagName: string) => {
    const newSet = new Set(expandedTags);
    if (newSet.has(tagName)) {
      newSet.delete(tagName);
    } else {
      newSet.add(tagName);
    }
    setExpandedTags(newSet);
  };

  // Use deltas if available, otherwise convert tags to delta format
  const displayDeltas: TagDelta[] = deltas || (tags || []).map(tag => ({
    tag: tag.name,
    content: tag.content,
    status: 'new' as const,
    previous_content: undefined,
  }));

  if (displayDeltas.length === 0) return null;

  const filteredDeltas = viewMode === 'changes-only' 
    ? displayDeltas.filter(d => d.status !== 'unchanged')
    : displayDeltas;

  if (filteredDeltas.length === 0) return null;

  const statusColors = {
    new: { bg: 'bg-green-500/10', border: 'border-green-500/30', text: 'text-green-400', badge: 'bg-green-600' },
    modified: { bg: 'bg-yellow-500/10', border: 'border-yellow-500/30', text: 'text-yellow-400', badge: 'bg-yellow-600' },
    removed: { bg: 'bg-red-500/10', border: 'border-red-500/30', text: 'text-red-400', badge: 'bg-red-600' },
    unchanged: { bg: 'bg-slate-800/20', border: 'border-slate-700/30', text: 'text-slate-400', badge: 'bg-slate-600' },
  };

  const changesCount = displayDeltas.filter(d => d.status !== 'unchanged').length;

  return (
    <div className="bg-slate-900/50 border border-slate-800 rounded-lg overflow-hidden mb-6">
      <div className="px-4 py-3 border-b border-slate-800 flex justify-between items-center bg-slate-900/80">
        <h3 className="text-xs font-bold text-slate-400 uppercase tracking-widest flex items-center gap-2">
          <Tag size={14} /> Message Tags
          {deltas && changesCount > 0 && (
            <span className="text-[10px] bg-yellow-600 px-2 py-0.5 rounded-full text-white font-mono">
              {changesCount} changed
            </span>
          )}
        </h3>
        <div className="flex items-center gap-2">
          {deltas && changesCount > 0 && (
            <button
              onClick={() => setViewMode(viewMode === 'all' ? 'changes-only' : 'all')}
              className="text-[10px] bg-slate-700 hover:bg-slate-600 px-2 py-1 rounded text-slate-300 font-mono transition-colors"
            >
              {viewMode === 'all' ? 'Show Changes Only' : 'Show All'}
            </button>
          )}
          <span className="text-[10px] bg-slate-800 px-2 py-0.5 rounded-full text-slate-500 font-mono">
            {filteredDeltas.length} tags
          </span>
        </div>
      </div>
      
      <div className="divide-y divide-slate-800">
        {filteredDeltas.map((delta, idx) => {
          const colors = statusColors[delta.status];
          return (
            <div key={`${delta.tag}-${idx}`} className={`hover:bg-slate-800/30 transition-colors ${colors.bg} border-l-2 ${colors.border}`}>
              <button
                onClick={() => toggleTag(delta.tag)}
                className="w-full text-left px-4 py-3 flex items-center justify-between group"
              >
                <div className="flex items-center gap-2">
                  <span className={`transition-transform ${expandedTags.has(delta.tag) ? 'rotate-90' : ''}`}>
                    ▶
                  </span>
                  <span className={`font-mono ${colors.text} font-semibold`}>&lt;{delta.tag}&gt;</span>
                  <span className={`text-[9px] ${colors.badge} px-1.5 py-0.5 rounded-full text-white font-bold uppercase`}>
                    {delta.status}
                  </span>
                </div>
                <span className="text-[10px] text-slate-500 font-mono">
                  {delta.content.length} chars
                </span>
              </button>
              
              {expandedTags.has(delta.tag) && (
                <div className="px-4 py-3 bg-slate-950/50 border-t border-slate-800">
                  {delta.status === 'modified' && delta.previous_content && (
                    <div className="mb-3 p-2 bg-red-950/30 border border-red-500/20 rounded">
                      <div className="text-[10px] text-red-400 font-bold mb-1 uppercase">Previous Content:</div>
                      <pre className="text-xs font-mono text-red-300/70 whitespace-pre-wrap break-words max-h-48 overflow-y-auto">
                        {delta.previous_content}
                      </pre>
                    </div>
                  )}
                  <div className={delta.status === 'modified' ? 'p-2 bg-green-950/30 border border-green-500/20 rounded' : ''}>
                    {delta.status === 'modified' && (
                      <div className="text-[10px] text-green-400 font-bold mb-1 uppercase">Current Content:</div>
                    )}
                    <pre className="text-xs font-mono text-slate-300 whitespace-pre-wrap break-words max-h-96 overflow-y-auto">
                      {delta.content}
                    </pre>
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function JsonTreeNode({ data, path = '' }: { data: unknown; path?: string }) {
  const [expanded, setExpanded] = useState(false);
  
  if (data === null) return <span className="text-slate-500">null</span>;
  if (typeof data === 'boolean') return <span className="text-blue-400">{data.toString()}</span>;
  if (typeof data === 'number') return <span className="text-yellow-400">{data}</span>;
  if (typeof data === 'string') {
    const isLong = data.length > 100;
    return (
      <span className="text-emerald-400">
        "{isLong ? data.slice(0, 100) + '...' : data}"
      </span>
    );
  }
  
  if (Array.isArray(data)) {
    const arr = data as unknown[];
    return (
      <div>
        <button
          onClick={() => setExpanded(!expanded)}
          className="text-slate-400 hover:text-slate-300 flex items-center gap-1 font-mono text-xs"
        >
          {expanded ? '▼' : '▶'} Array [{arr.length}]
        </button>
        {expanded && (
          <div className="ml-4 border-l border-slate-700 pl-3 space-y-1">
            {data.map((item, idx) => (
              <div key={idx} className="text-slate-400">
                <span className="text-slate-600">[{idx}]:</span>{' '}
                <JsonTreeNode data={item} path={`${path}[${idx}]`} />
              </div>
            ))}
          </div>
        )}
      </div>
    );
  }
  
  if (typeof data === 'object') {
    const obj = data as Record<string, unknown>;
    const keys = Object.keys(obj);
    return (
      <div>
        <button
          onClick={() => setExpanded(!expanded)}
          className="text-slate-400 hover:text-slate-300 flex items-center gap-1 font-mono text-xs"
        >
          {expanded ? '▼' : '▶'} Object {`{${keys.length}}`}
        </button>
        {expanded && (
          <div className="ml-4 border-l border-slate-700 pl-3 space-y-1">
            {keys.map(key => (
              <div key={key} className="text-slate-400">
                <span className="text-blue-300">{key}:</span>{' '}
                <JsonTreeNode data={obj[key]} path={`${path}.${key}`} />
              </div>
            ))}
          </div>
        )}
      </div>
    );
  }
  
  return <span className="text-slate-500">unknown</span>;
}



type JsonMatch = {
  needle: string;
  path: string;
  startIndex: number;
  endIndex: number;
  expandPaths: string[]; // container paths to expand so the node becomes visible
};

function findFirstStringMatch(
  data: unknown,
  needle: string,
  path: string = '',
  containers: string[] = ['']
): JsonMatch | null {
  if (typeof data === 'string') {
    const idx = data.indexOf(needle);
    if (idx >= 0) {
      // Dedup but preserve order
      const expandPaths: string[] = [];
      for (const p of containers) {
        if (!expandPaths.includes(p)) expandPaths.push(p);
      }
      return {
        needle,
        path,
        startIndex: idx,
        endIndex: idx + needle.length,
        expandPaths,
      };
    }
    return null;
  }

  if (data === null || typeof data !== 'object') return null;

  if (Array.isArray(data)) {
    const arr = data as unknown[];
    const nextContainers = path ? [...containers, path] : containers;
    for (let i = 0; i < arr.length; i++) {
      const childPath = `${path}[${i}]`;
      const res = findFirstStringMatch(arr[i], needle, childPath, nextContainers);
      if (res) return res;
    }
    return null;
  }

  // object
  const nextContainers = path ? [...containers, path] : containers;
  const obj = data as Record<string, unknown>;
  for (const key of Object.keys(obj)) {
    const childPath = path ? `${path}.${key}` : `.${key}`;
    const res = findFirstStringMatch(obj[key], needle, childPath, nextContainers);
    if (res) return res;
  }

  return null;
}

function JsonTreeNodeControlled(props: {
  data: unknown;
  path?: string;
  expandedPaths: Set<string>;
  onToggle: (path: string) => void;
  highlight: JsonMatch | null;
}) {
  const { data, path = '', expandedPaths, onToggle, highlight } = props;

  if (data === null) return <span className="text-slate-500">null</span>;
  if (typeof data === 'boolean') return <span className="text-blue-400">{data.toString()}</span>;
  if (typeof data === 'number') return <span className="text-yellow-400">{data}</span>;

  if (typeof data === 'string') {
    const isTarget = highlight && highlight.path === path;

    if (!isTarget) {
      const isLong = data.length > 100;
      return <span className="text-emerald-400">"{isLong ? data.slice(0, 100) + '...' : data}"</span>;
    }

    const before = data.slice(0, highlight.startIndex);
    const mid = data.slice(highlight.startIndex, highlight.endIndex);
    const after = data.slice(highlight.endIndex);

    return (
      <span className="text-emerald-400" data-json-path={path}>
        "{before}
        <span className="bg-emerald-500/20 text-emerald-200 px-0.5 rounded border border-emerald-500/30">
          {mid}
        </span>
        {after}"
      </span>
    );
  }

  if (Array.isArray(data)) {
    const arr = data as unknown[];
    const expanded = expandedPaths.has(path);
    return (
      <div>
        <button
          onClick={() => onToggle(path)}
          className="text-slate-400 hover:text-slate-300 flex items-center gap-1 font-mono text-xs"
        >
          {expanded ? '▼' : '▶'} Array [{arr.length}]
        </button>
        {expanded && (
          <div className="ml-4 border-l border-slate-700 pl-3 space-y-1">
            {arr.map((item, idx) => {
              const childPath = `${path}[${idx}]`;
              return (
                <div key={childPath} className="text-slate-400">
                  <span className="text-slate-600">[{idx}]:</span>{' '}
                  <JsonTreeNodeControlled
                    data={item}
                    path={childPath}
                    expandedPaths={expandedPaths}
                    onToggle={onToggle}
                    highlight={highlight}
                  />
                </div>
              );
            })}
          </div>
        )}
      </div>
    );
  }

  if (typeof data === 'object') {
    const obj = data as Record<string, unknown>;
    const expanded = expandedPaths.has(path);
    const keys = Object.keys(obj);
    return (
      <div>
        <button
          onClick={() => onToggle(path)}
          className="text-slate-400 hover:text-slate-300 flex items-center gap-1 font-mono text-xs"
        >
          {expanded ? '▼' : '▶'} Object {`{${keys.length}}`}
        </button>
        {expanded && (
          <div className="ml-4 border-l border-slate-700 pl-3 space-y-1">
            {keys.map((key) => {
              const childPath = path ? `${path}.${key}` : `.${key}`;
              return (
                <div key={childPath} className="text-slate-400">
                  <span className="text-blue-300">{key}:</span>{' '}
                  <JsonTreeNodeControlled
                    data={obj[key]}
                    path={childPath}
                    expandedPaths={expandedPaths}
                    onToggle={onToggle}
                    highlight={highlight}
                  />
                </div>
              );
            })}
          </div>
        )}
      </div>
    );
  }

  return <span className="text-slate-500">unknown</span>;
}

function App() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [selectedCid, setSelectedCid] = useState<string | null>(null);
  const [conversation, setConversation] = useState<Conversation | null>(null);
  const [selectedTurn, setSelectedTurn] = useState<string | null>(null);
  const [turnDetail, setTurnDetail] = useState<TurnDetail | null>(null);
  const [filterText, setFilterText] = useState<string>('');
  const [selectedToolCall, setSelectedToolCall] = useState<ToolCallIndex | null>(null);
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  const [selectedStage, setSelectedStage] = useState<Stage | null>(null);
  const [expandedSummaries, setExpandedSummaries] = useState<Set<string>>(new Set());
  const [expandedToolCalls, setExpandedToolCalls] = useState<Set<string>>(new Set());
  const [selectedBlob, setSelectedBlob] = useState<{ content: string; contentType: string } | null>(null);
  const [visibleBlobId, setVisibleBlobId] = useState<string | null>(null);
  const [blobViewMode, setBlobViewMode] = useState<'formatted' | 'escaped' | 'structured'>('formatted');
  const [expandedJsonPaths, setExpandedJsonPaths] = useState<Set<string>>(new Set(['']));
  const [pendingBlobNeedle, setPendingBlobNeedle] = useState<string | null>(null);
  const [blobHighlight, setBlobHighlight] = useState<JsonMatch | null>(null);
  const [selectedLeakTag, setSelectedLeakTag] = useState<TagOccurrence | null>(null);
  const [compareStages, setCompareStages] = useState<[string | null, string | null]>([null, null]);
  const [blobContent, setBlobContent] = useState<{ [key: string]: string }>({});
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  const [_loading, setLoading] = useState(false);
  const [stageDiff, setStageDiff] = useState<StageDiffResponse | null>(null);
  const [stageDiffLoading, setStageDiffLoading] = useState(false);

  useEffect(() => {
    fetchConversations();
  }, []);

  useEffect(() => {
    // Fetch diff when both stages are selected
    if (compareStages[0] && compareStages[1] && selectedCid && selectedTurn) {
      void (async () => {
        setStageDiffLoading(true);
        try {
          const res = await axios.post(`${API_BASE}/diff/${selectedCid}/${selectedTurn}`, {
            stage1: compareStages[0],
            stage2: compareStages[1],
          });
          setStageDiff(res.data);
        } catch (err) {
          console.error("Failed to fetch stage diff", err);
          setStageDiff(null);
        } finally {
          setStageDiffLoading(false);
        }
      })();
    } else {
      setStageDiff(null);
    }
  }, [compareStages, selectedCid, selectedTurn]);

  const parsedBlobJson = useMemo(() => {
    if (!selectedBlob) return null;
    try {
      return JSON.parse(selectedBlob.content);
    } catch {
      return null;
    }
  }, [selectedBlob]);

  useEffect(() => {
    if (!parsedBlobJson || !pendingBlobNeedle) return;

    const match = findFirstStringMatch(parsedBlobJson, pendingBlobNeedle);
    if (match) {
      setBlobHighlight(match);
      setExpandedJsonPaths(new Set(match.expandPaths));
    }
    setPendingBlobNeedle(null);
  }, [parsedBlobJson, pendingBlobNeedle]);

  useEffect(() => {
    if (!blobHighlight) return;

    requestAnimationFrame(() => {
      const el = document.querySelector(`[data-json-path="${blobHighlight.path}"]`);
      if (el) {
        (el as HTMLElement).scrollIntoView({ block: 'center', behavior: 'smooth' });
      }
    });
  }, [blobHighlight]);


  const fetchConversations = async () => {
    try {
      const res = await axios.get(`${API_BASE}/conversations`);
      setConversations(res.data);
    } catch (err) {
      console.error("Failed to fetch conversations", err);
    }
  };

  const selectConversation = async (cid: string) => {
    setSelectedCid(cid);
    setSelectedTurn(null);
    setTurnDetail(null);
    setLoading(true);
    try {
      const res = await axios.get(`${API_BASE}/conversation/${cid}`);
      setConversation(res.data);
    } catch (err) {
      console.error("Failed to fetch conversation", err);
    } finally {
      setLoading(false);
    }
  };

  const selectTurn = async (tid: string) => {
    setSelectedTurn(tid);
    setSelectedToolCall(null);
    setSelectedStage(null);
    setLoading(true);
    try {
      const res = await axios.get(`${API_BASE}/conversation/${selectedCid}/turn/${tid}`);
      setTurnDetail(res.data);
    } catch (err) {
      console.error("Failed to fetch turn", err);
    } finally {
      setLoading(false);
    }
  };

  const filterTurns = (turns: TurnSummary[], filter: string): TurnSummary[] => {
    if (!filter.trim()) return turns;
    
    const lowerFilter = filter.toLowerCase();
    return turns.filter(turn => {
      // Check basic fields
      if (turn.request_id.toLowerCase().includes(lowerFilter)) return true;
      if (turn.model_id.toLowerCase().includes(lowerFilter)) return true;
      if (turn.flavor.toLowerCase().includes(lowerFilter)) return true;
      
      // Check issue keywords
      if (lowerFilter.includes('issue:')) {
        if (lowerFilter.includes('issue:tool_args_empty') && turn.issues.tool_args_empty > 0) return true;
        if (lowerFilter.includes('issue:repaired') && turn.issues.tool_args_repaired > 0) return true;
        if (lowerFilter.includes('issue:rescue') && turn.issues.rescue_used > 0) return true;
        if (lowerFilter.includes('issue:tag') && (turn.issues.tag_unregistered > 0 || turn.issues.tag_leak_echo > 0)) return true;
      }
      
      return false;
    });
  };

  const toggleSummaryExpanded = (stageName: string) => {
    const newSet = new Set(expandedSummaries);
    if (newSet.has(stageName)) {
      newSet.delete(stageName);
    } else {
      newSet.add(stageName);
    }
    setExpandedSummaries(newSet);
  };

  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  const renderSummaryValue = (value: unknown): string => {
    if (typeof value === 'string' || typeof value === 'number') return String(value);
    if (typeof value === 'boolean') return value ? 'true' : 'false';
    return JSON.stringify(value);
  };

  const fetchBlob = async (cid: string, tid: string, blobId: string) => {
    const cacheKey = `${cid}/${tid}/${blobId}`;
    if (blobContent[cacheKey]) {
      setSelectedBlob({ content: blobContent[cacheKey], contentType: 'application/json' });
      return;
    }
    try {
      const res = await axios.get(`${API_BASE}/blob/${cid}/${tid}/${blobId}`);
      const content = typeof res.data === 'string' ? res.data : JSON.stringify(res.data, null, 2);
      setBlobContent(prev => ({ ...prev, [cacheKey]: content }));
      setSelectedBlob({ content, contentType: res.headers['content-type'] || 'application/json' });
    } catch (err) {
      console.error("Failed to fetch blob", err);
      setSelectedBlob({ content: 'Failed to load blob', contentType: 'text/plain' });
    }
  };

  const openBlobStructuredAtNeedle = async (blobId: string, needle: string) => {
    if (!selectedCid || !selectedTurn) return;

    setBlobViewMode('structured');
    setPendingBlobNeedle(needle);
    setBlobHighlight(null);

    await fetchBlob(selectedCid, selectedTurn, blobId);
  };


  const jumpToEvidence = (tc: ToolCallIndex) => {
    if (selectedCid && selectedTurn && tc.evidence.blob_ref) {
      fetchBlob(selectedCid, selectedTurn, tc.evidence.blob_ref.blob_id);
      setSelectedToolCall(tc);
    }
  };

  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  const jumpToLeakEvidence = (location: TagLocation) => {
    if (selectedCid && selectedTurn && location.blob_ref) {
      fetchBlob(selectedCid, selectedTurn, location.blob_ref.blob_id);
    }
  };

  const jumpToToolResult = (tr: ToolResultIndex) => {
    if (selectedCid && selectedTurn && tr.blob_ref) {
      fetchBlob(selectedCid, selectedTurn, tr.blob_ref.blob_id);
    }
  };

  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  const formatBytes = (bytes: number): string => {
    if (bytes < 1024) return `${bytes}B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)}MB`;
  };

  const formatDuration = (ms: number): string => {
    if (ms < 1000) return `${ms}ms`;
    if (ms < 60000) return `${(ms / 1000).toFixed(2)}s`;
    const minutes = Math.floor(ms / 60000);
    const seconds = ((ms % 60000) / 1000).toFixed(2);
    return `${minutes}m ${seconds}s`;
  };

  return (
    <div className="h-screen w-screen bg-slate-950 text-slate-200 font-sans overflow-hidden">
      <PanelGroup orientation="horizontal" className="h-full w-full">
        {/* Sidebar: Conversation List */}
        <Panel defaultSize={20} minSize={15} className="bg-slate-900 border-r border-slate-800 flex flex-col h-full">
          <div className="p-4 pt-6 border-b border-slate-800 flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Terminal size={20} className="text-emerald-400" />
              <h1 className="text-lg font-bold text-emerald-500">Parallax Debug</h1>
            </div>
            <span className="text-xs bg-slate-800 px-2 py-1 rounded text-slate-400">v0.2.0</span>
          </div>
          <div className="flex-1 overflow-y-auto p-3 space-y-2">
            {conversations.map(conv => (
              <button
                key={conv.conversation_id}
                onClick={() => selectConversation(conv.conversation_id)}
                className={`w-full text-left p-3 rounded-lg transition-all border ${selectedCid === conv.conversation_id ? 'bg-emerald-900/20 border-emerald-500/50 shadow-md shadow-emerald-900/10' : 'bg-slate-800/50 hover:bg-slate-800 border-slate-700/50 hover:border-slate-600'}`}
              >
                <div className="flex justify-between items-center mb-1.5">
                  <span className="font-mono text-sm font-bold text-slate-200">{conv.conversation_id.slice(0, 8)}</span>
                  <span className="text-[10px] text-slate-500 font-mono bg-slate-900/50 px-1.5 py-0.5 rounded">
                    {new Date(conv.last_updated_ms).toLocaleTimeString([], {hour: '2-digit', minute:'2-digit', second:'2-digit'})}
                  </span>
                </div>
                <div className="flex items-center gap-2">
                  <span className="text-xs text-slate-400 font-medium">{conv.turns.length} turns</span>
                  <div className="flex gap-1">
                    {conv.issues.reasoning_leak > 0 && <span className="w-2 h-2 rounded-full bg-red-500 ring-1 ring-red-900/50" title="Reasoning Leak"></span>}
                    {(conv.issues.tool_args_empty > 0 || conv.issues.tool_args_repaired > 0) && <span className="w-2 h-2 rounded-full bg-yellow-500 ring-1 ring-yellow-900/50" title="Tool Issue"></span>}
                    {conv.issues.tag_leak_echo > 0 && <span className="w-2 h-2 rounded-full bg-orange-500 ring-1 ring-orange-900/50" title="Tag Leak"></span>}
                  </div>
                </div>
              </button>
            ))}
          </div>
        </Panel>
        
        <PanelResizeHandle className="w-1 bg-slate-800 hover:bg-emerald-500 transition-colors cursor-col-resize z-10" />

        {/* Main Content */}
        <Panel minSize={30}>
          <div className="flex h-full overflow-hidden bg-slate-950">
            {selectedCid ? (
              <PanelGroup orientation="horizontal">
                {/* Turn List */}
                <Panel defaultSize={30} minSize={20} className="bg-slate-900/50 border-r border-slate-800 flex flex-col h-full">
                  <div className="p-4 pt-6 border-b border-slate-800 flex items-center justify-between bg-slate-900/80 backdrop-blur-sm sticky top-0 z-10">
                    <h2 className="text-sm font-bold uppercase tracking-wider text-slate-400">Turns</h2>
                    {conversation && <span className="text-xs text-slate-500 font-mono bg-slate-800 px-2 py-0.5 rounded">{conversation.conversation_id.slice(0, 8)}</span>}
                  </div>
                  <div className="p-3 border-b border-slate-800 bg-slate-900/30">
                    <div className="relative">
                      <Search size={14} className="absolute left-2.5 top-2.5 text-slate-500" />
                      <input
                        type="text"
                        placeholder="Filter turns..."
                        value={filterText}
                        onChange={(e) => setFilterText(e.target.value)}
                        className="w-full pl-8 pr-3 py-2 bg-slate-800 border border-slate-700 rounded-lg text-xs text-slate-200 placeholder-slate-500 focus:outline-none focus:border-emerald-500 focus:ring-1 focus:ring-emerald-500 transition-all"
                      />
                    </div>
                  </div>
                  <div className="flex-1 overflow-y-auto p-3 space-y-2.5">
                    {conversation && filterTurns(conversation.turns, filterText).map((turn, idx) => (
                      <button
                        key={turn.turn_id}
                        onClick={() => selectTurn(turn.turn_id)}
                        className={`w-full text-left p-3.5 rounded-xl transition-all border group relative overflow-hidden ${selectedTurn === turn.turn_id ? 'bg-slate-800 border-emerald-500/50 ring-1 ring-emerald-500/30 shadow-lg' : 'bg-slate-800/40 hover:bg-slate-800 border-slate-700/50 hover:border-slate-600 shadow-sm'}`}
                      >
                        {selectedTurn === turn.turn_id && <div className="absolute left-0 top-0 bottom-0 w-1 bg-emerald-500"></div>}
                        <div className="flex items-center justify-between mb-2.5">
                          <div className="flex items-center gap-2 flex-wrap">
                            <div className="px-1.5 py-0.5 bg-emerald-950/30 border border-emerald-500/20 rounded text-emerald-400 font-mono text-[10px] font-bold min-w-[2rem] text-center">
                              #{idx + 1}
                            </div>
                            <div className={`px-1.5 py-0.5 rounded border flex items-center gap-1 ${
                              turn.role === 'Assistant' ? 'bg-purple-950/30 border-purple-500/20 text-purple-400' :
                              turn.role === 'User' ? 'bg-blue-950/30 border-blue-500/20 text-blue-400' :
                              'bg-slate-950/30 border-slate-500/20 text-slate-400'
                            }`}>
                              <span className="text-[10px] font-bold font-mono">
                                {turn.role || (idx % 2 === 0 ? 'Cursor' : 'Assistant')}
                              </span>
                            </div>
                            <div className="px-1.5 py-0.5 bg-slate-800 border border-slate-700 rounded text-slate-400 font-mono text-[10px] truncate max-w-[80px]" title={turn.model_id}>
                              {turn.model_id.split('/').pop()}
                            </div>
                            {/* Issue indicators */}
                            {turn.issues && (
                              <div className="flex gap-1">
                                {turn.issues.reasoning_leak > 0 && <span className="w-1.5 h-1.5 rounded-full bg-red-500" title="Reasoning Leak"></span>}
                                {turn.issues.tool_args_invalid > 0 && <span className="w-1.5 h-1.5 rounded-full bg-red-400" title="Invalid Tool Args"></span>}
                                {turn.issues.tool_args_repaired > 0 && <span className="w-1.5 h-1.5 rounded-full bg-yellow-500" title="Repaired Tool Args"></span>}
                                {turn.issues.tool_args_empty > 0 && <span className="w-1.5 h-1.5 rounded-full bg-yellow-400" title="Empty Tool Args"></span>}
                                {turn.issues.tag_leak_echo > 0 && <span className="w-1.5 h-1.5 rounded-full bg-orange-500" title="Tag Leak"></span>}
                                {turn.issues.tag_unregistered > 0 && <span className="w-1.5 h-1.5 rounded-full bg-orange-400" title="Unregistered Tag"></span>}
                                {turn.issues.rescue_used > 0 && <span className="w-1.5 h-1.5 rounded-full bg-amber-500" title="Rescue Used"></span>}
                                {turn.issues.tool_call_duplicate_id > 0 && <span className="w-1.5 h-1.5 rounded-full bg-pink-500" title="Duplicate Tool Call ID"></span>}
                              </div>
                            )}
                          </div>
                          <span className="text-[10px] text-slate-500 font-mono opacity-0 group-hover:opacity-100 transition-opacity">{turn.request_id.slice(0, 8)}</span>
                        </div>
                        <div className="flex justify-between items-center text-[10px] text-slate-400 font-medium">
                          <span className="bg-slate-800/50 px-1.5 py-0.5 rounded text-slate-500">{turn.flavor}</span>
                          <span className="font-mono text-slate-500">{new Date(turn.started_at_ms).toLocaleTimeString()}</span>
                        </div>
                      </button>
                    ))}
                  </div>
                </Panel>


                <PanelResizeHandle className="w-1 bg-slate-800 hover:bg-emerald-500 transition-colors cursor-col-resize" />

                {/* Turn View */}
                <Panel defaultSize={70} minSize={30} className="bg-slate-950 flex flex-col h-full overflow-hidden">
              {turnDetail ? (
                <div className="flex flex-col h-full overflow-hidden">
                  <div className="flex-1 overflow-y-auto">
                  <div className="p-6 pt-8 border-b border-slate-800 bg-slate-900/30">
                    <div className="flex flex-wrap justify-between items-start gap-4 mb-6">
                      <div className="flex-1 min-w-[200px]">
                        <h2 className="text-2xl font-bold text-slate-100 mb-2">Turn Details</h2>
                        <div className="flex flex-wrap items-center gap-4 text-xs text-slate-400 font-mono">
                          <div className="flex items-center gap-2 bg-slate-900/50 px-2 py-1 rounded border border-slate-800">
                            <span className="text-slate-500">RID:</span>
                            <span className="text-emerald-400">{turnDetail.request_id}</span>
                          </div>
                          <div className="flex items-center gap-2 bg-slate-900/50 px-2 py-1 rounded border border-slate-800">
                            <span className="text-slate-500">Model:</span>
                            <span className="text-emerald-400">{turnDetail.model_id}</span>
                          </div>
                          <div className="flex items-center gap-2 bg-slate-900/50 px-2 py-1 rounded border border-slate-800">
                             <span className="text-slate-500">Duration:</span>
                             <span className="text-slate-300">{turnDetail.ended_at_ms ? formatDuration(turnDetail.ended_at_ms - turnDetail.started_at_ms) : '...'}</span>
                          </div>
                        </div>
                      </div>
                    </div>

                  {/* User Query Section */}
                  {turnDetail.user_query && (
                    <div className="mb-6 bg-blue-950/20 border border-blue-500/30 rounded-lg p-4">
                      <h3 className="text-xs font-bold text-blue-400 uppercase tracking-widest mb-2 flex items-center gap-2">
                        <Terminal size={14} /> User Query
                      </h3>
                      <div className="text-sm text-slate-200 font-mono bg-slate-900/50 p-3 rounded border border-slate-700 whitespace-pre-wrap">
                        {turnDetail.user_query}
                      </div>
                    </div>
                  )}

                  {/* Message Tags Section */}
                  {(turnDetail.user_query_tags || turnDetail.user_query) && (
                    <TagsDisplay deltas={turnDetail.user_query_tags} />
                  )}

                  {/* Trace Timeline */}
                  {turnDetail.span_summary && turnDetail.span_summary.length > 0 && (
                    <div className="mb-6">

                      <h3 className="text-sm font-bold uppercase text-cyan-400 mb-3">Trace Timeline</h3>
                      <div className="space-y-2">
                        {turnDetail.span_summary.map((span, idx) => (
                          <div key={idx} className="p-3 bg-slate-800/30 border border-slate-700 rounded">
                            <div className="flex items-center justify-between mb-1">
                              <span className="font-mono text-cyan-300 font-semibold">{span.name}</span>
                              <span className={`text-xs px-2 py-0.5 rounded ${
                                span.level === 'INFO' ? 'bg-blue-900/50 text-blue-300' :
                                span.level === 'DEBUG' ? 'bg-slate-700/50 text-slate-300' :
                                'bg-yellow-900/50 text-yellow-300'
                              }`}>
                                {span.level}
                              </span>
                            </div>
                            {(() => {
                              const fields = span.fields;
                              if (!fields || typeof fields !== "object" || Array.isArray(fields)) return null;
                              const entries = Object.entries(fields as Record<string, unknown>);
                              if (entries.length === 0) return null;
                              return (
                              <div className="text-xs font-mono text-slate-400 space-y-1 ml-2">
                                {entries.slice(0, 3).map(([key, value]) => (
                                  <div key={key}>
                                    <span className="text-slate-500">{key}:</span> <span className="text-slate-300">{String(value).slice(0, 50)}</span>
                                  </div>
                                ))}
                              </div>
                              );
                            })()}
                          </div>
                        ))}
                      </div>
                    </div>
                  )}

                    {/* Issues Section */}
                    {turnDetail.issues.length > 0 && (
                      <div className="bg-red-950/20 border border-red-500/30 rounded-lg p-4 mb-2">
                        <h3 className="text-xs font-bold text-red-400 uppercase tracking-widest mb-3">Issues Detected</h3>
                        <div className="space-y-2">
                          {turnDetail.issues.map((issue, i) => (
                            <div key={i} className="flex items-start gap-2 text-sm">
                              <span className="px-1.5 py-0.5 bg-red-900/40 rounded text-[10px] font-bold text-red-400 mt-0.5">{issue.kind}</span>
                              <span className="text-slate-300">{issue.message}</span>
                            </div>
                          ))}
                        </div>
                      </div>
                    )}

                  {/* Tool Calls Section */}
                  {turnDetail.tool_calls.length > 0 && (
                    <div className="mb-6">
                      <h3 className="text-sm font-bold uppercase text-blue-400 mb-3 flex items-center gap-2">
                        <Wrench size={16} /> 
                        {turnDetail.role === 'User' ? 'Input Tool Intents (Rescued)' : 'Model Tool Calls'} ({turnDetail.tool_calls.length})
                      </h3>
                      <div className="overflow-x-auto max-h-64 border border-slate-800 rounded-lg">
                        <table className="w-full text-xs text-left border-collapse">
                          <thead className="text-xs text-slate-400 uppercase bg-slate-900/95 sticky top-0 backdrop-blur-sm z-10 shadow-sm">
                            <tr>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Name</th>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">ID</th>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Status</th>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Args</th>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Origin</th>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Stage</th>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Action</th>
                            </tr>
                          </thead>
                          <tbody className="divide-y divide-slate-800">
                            {turnDetail.tool_calls.map((tc, idx) => {
                              const isExpanded = expandedToolCalls.has(tc.id);
                              const argsPreview = formatArgsPreview(tc.evidence.raw_arguments_snippet || tc.evidence.snippet);

                              return (
                                <tr key={idx} className={`bg-slate-900/50 hover:bg-slate-800/80 transition-colors ${idx % 2 === 1 ? 'bg-slate-900/30' : ''}`}>
                                  <td className="px-4 py-2 font-mono text-blue-300 font-medium">{tc.name}</td>
                                  <td className="px-4 py-2 font-mono text-slate-500 text-[10px]" title={tc.id}>
                                    {tc.id.length > 8 ? tc.id.slice(0, 8) + '...' : tc.id}
                                  </td>
                                  <td className="px-4 py-2">
                                    <span className={`inline-flex items-center px-2 py-0.5 rounded text-[10px] font-medium border ${
                                      tc.args_status === 'Ok' ? 'bg-green-950/40 border-green-900/50 text-green-400' :
                                      tc.args_status === 'Empty' ? 'bg-red-950/40 border-red-900/50 text-red-400' :
                                      tc.args_status === 'Invalid' ? 'bg-red-950/40 border-red-900/50 text-red-400' :
                                      'bg-yellow-950/40 border-yellow-900/50 text-yellow-400'
                                    }`}>
                                      {tc.args_status}
                                    </span>
                                  </td>
                                  <td className="px-4 py-2">
                                    <div className="flex items-center gap-2">
                                      <code className="text-slate-400 text-[10px] font-mono truncate max-w-xs" title={tc.evidence.snippet || 'No args'}>
                                        {argsPreview}
                                      </code>
                                      {tc.evidence.raw_arguments_snippet && (
                                        <button
                                          onClick={() => {
                                            const newExpanded = new Set(expandedToolCalls);
                                            if (newExpanded.has(tc.id)) {
                                              newExpanded.delete(tc.id);
                                            } else {
                                              newExpanded.add(tc.id);
                                            }
                                            setExpandedToolCalls(newExpanded);
                                            setSelectedToolCall(isExpanded ? null : tc);
                                          }}
                                          className="text-blue-400 hover:text-blue-300 flex-shrink-0 p-1 hover:bg-blue-900/20 rounded transition-colors"
                                          title={isExpanded ? 'Collapse' : 'Expand'}
                                        >
                                          {isExpanded ? '▼' : '▶'}
                                        </button>
                                      )}
                                    </div>
                                  </td>
                                  <td className="px-4 py-2">
                                    <span className={`px-1.5 py-0.5 rounded text-[9px] font-bold uppercase ${
                                      tc.origin === 'ingress' ? 'bg-blue-900/30 text-blue-400 border border-blue-500/20' :
                                      tc.origin === 'upstream_stream' ? 'bg-purple-900/30 text-purple-400 border border-purple-500/20' :
                                      'bg-slate-800 text-slate-500'
                                    }`}>
                                      {tc.origin.replace('_', ' ')}
                                    </span>
                                  </td>
                                  <td className="px-4 py-2 text-slate-400">{tc.evidence.stage}</td>
                                  <td className="px-4 py-2">
                                    {tc.evidence.blob_ref && (
                                      <button
                                        onClick={() => jumpToEvidence(tc)}
                                        className="text-blue-400 hover:text-blue-300 flex items-center gap-1 p-1 hover:bg-blue-900/20 rounded transition-colors"
                                        title="Jump to evidence"
                                      >
                                        <ExternalLink size={14} />
                                      </button>
                                    )}
                                  </td>
                                </tr>
                              );
                            })}
                          </tbody>
                        </table>
                      </div>
                      {selectedToolCall && (
                        <div className="mt-4 p-4 bg-slate-800/50 border border-slate-700 rounded-lg">
                          <h4 className="text-sm font-bold text-slate-300 mb-4 border-b border-slate-700 pb-2">Tool Call Analysis: {selectedToolCall.name}</h4>
                          
                          {/* Metadata Grid */}
                          <div className="grid grid-cols-2 gap-4 mb-6 text-xs">
                            <div>
                              <span className="text-slate-500 block mb-1">ID</span>
                              <span className="font-mono text-slate-300 bg-slate-900/50 px-2 py-1 rounded">{selectedToolCall.id}</span>
                            </div>
                            <div>
                              <span className="text-slate-500 block mb-1">Status</span>
                              <span className={`inline-flex px-2 py-1 rounded font-bold ${
                                selectedToolCall.args_status === 'Ok' ? 'text-green-400 bg-green-950/30 border border-green-900/50' : 
                                'text-yellow-400 bg-yellow-950/30 border border-yellow-900/50'
                              }`}>{selectedToolCall.args_status}</span>
                            </div>
                          </div>

                          {/* Arguments Table */}
                          {(() => {
                            try {
                              // Use raw_arguments_snippet for the full JSON data
                              const argsSource = selectedToolCall.evidence.raw_arguments_snippet || selectedToolCall.evidence.snippet;
                              if (!argsSource) return <div className="text-slate-500 italic">No argument data available</div>;
                              
                              let args: Record<string, unknown> = {};
                              try {
                                const parsedArgs: unknown = JSON.parse(argsSource);
                                if (parsedArgs && typeof parsedArgs === 'object' && !Array.isArray(parsedArgs)) {
                                  args = parsedArgs as Record<string, unknown>;
                                } else {
                                  return <div className="text-red-400">Tool arguments JSON is not an object</div>;
                                }
                              } catch {
                                return (
                                   <div className="space-y-2">
                                      <div className="text-red-400 text-xs font-bold">Failed to parse JSON arguments</div>
                                      <pre className="text-xs font-mono bg-slate-950 p-3 rounded border border-red-900/30 text-red-300 whitespace-pre-wrap">
                                        {argsSource}
                                      </pre>
                                   </div>
                                );
                              }

                              return (
                                <div className="border border-slate-700 rounded-lg overflow-hidden">
                                   <table className="w-full text-xs text-left">
                                     <thead className="bg-slate-900 text-slate-400 font-semibold border-b border-slate-700">
                                       <tr>
                                         <th className="px-3 py-2 w-1/4">Argument</th>
                                         <th className="px-3 py-2">Value</th>
                                       </tr>
                                     </thead>
                                     <tbody className="divide-y divide-slate-800 bg-slate-900/30">
                                       {Object.entries(args).map(([key, value]) => (
                                         <tr key={key} className="hover:bg-slate-800/50">
                                           <td className="px-3 py-2 font-mono text-blue-300 align-top pt-3">{key}</td>
                                           <td className="px-3 py-2 font-mono text-slate-300">
                                             <pre className="whitespace-pre-wrap max-h-60 overflow-y-auto font-mono text-[11px] leading-relaxed">
                                               {typeof value === 'string' ? value : JSON.stringify(value, null, 2)}
                                             </pre>
                                           </td>
                                         </tr>
                                       ))}
                                     </tbody>
                                   </table>
                                </div>
                              );
                            } catch {
                              return <div className="text-red-400">Error rendering arguments</div>;
                            }
                          })()}
                        </div>
                      )}
                    </div>
                  )}

                  {/* Tool Results Section */}
                  {turnDetail.tool_results && turnDetail.tool_results.length > 0 && (
                    <div className="mb-6">
                      <h3 className="text-sm font-bold uppercase text-blue-400 mb-3 flex items-center gap-2">
                        <Terminal size={16} /> 
                        Tool Outputs ({turnDetail.tool_results.length})
                      </h3>
                      <div className="overflow-x-auto max-h-64 border border-slate-800 rounded-lg">
                        <table className="w-full text-xs text-left border-collapse">
                          <thead className="text-xs text-slate-400 uppercase bg-slate-900/95 sticky top-0 backdrop-blur-sm z-10 shadow-sm">
                            <tr>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Tool Name</th>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Call ID</th>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Status</th>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Content Snippet</th>
                              <th className="px-4 py-3 font-semibold border-b border-slate-700">Action</th>
                            </tr>
                          </thead>
                          <tbody className="divide-y divide-slate-800">
                            {turnDetail.tool_results.map((tr, idx) => (
                              <tr key={idx} className={`bg-slate-900/50 hover:bg-slate-800/80 transition-colors ${idx % 2 === 1 ? 'bg-slate-900/30' : ''}`}>
                                <td className="px-4 py-2 font-mono text-blue-300 font-medium">
                                  {tr.name || <span className="text-slate-600 italic">unknown</span>}
                                </td>
                                <td className="px-4 py-2 font-mono text-slate-500 text-[10px]" title={tr.tool_call_id}>
                                  {tr.tool_call_id.length > 8 ? tr.tool_call_id.slice(0, 8) + '...' : tr.tool_call_id}
                                </td>
                                <td className="px-4 py-2">
                                  <span className={`inline-flex items-center px-2 py-0.5 rounded text-[10px] font-medium border ${
                                    tr.is_error ? 'bg-red-950/40 border-red-900/50 text-red-400' :
                                    'bg-green-950/40 border-green-900/50 text-green-400'
                                  }`}>
                                    {tr.is_error ? 'Error' : 'Success'}
                                  </span>
                                </td>
                                <td className="px-4 py-2">
                                  <code className="text-slate-400 text-[10px] font-mono block max-w-md truncate" title={tr.snippet}>
                                    {tr.snippet}
                                  </code>
                                </td>
                                <td className="px-4 py-2">
                                  {tr.blob_ref && (
                                    <button
                                      onClick={() => jumpToToolResult(tr)}
                                      className="text-blue-400 hover:text-blue-300 flex items-center gap-1 p-1 hover:bg-blue-900/20 rounded transition-colors"
                                      title="Jump to full output"
                                    >
                                      <ExternalLink size={14} />
                                    </button>
                                  )}
                                </td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      </div>
                    </div>
                  )}

                    {/* Tags Section */}
                    {(turnDetail.cursor_tags.registered.length > 0 || turnDetail.cursor_tags.unregistered.length > 0 || turnDetail.cursor_tags.leaks.length > 0) && (
                      <div className="bg-slate-900/50 border border-slate-800 rounded-lg overflow-hidden mb-6">
                        <div className="px-4 py-3 border-b border-slate-800 flex justify-between items-center bg-slate-900/80">
                          <h3 className="text-xs font-bold text-slate-400 uppercase tracking-widest flex items-center gap-2">
                            <Tag size={14} /> Cursor Tags
                          </h3>
                          <span className="text-[10px] bg-slate-800 px-2 py-0.5 rounded-full text-slate-500 font-mono">
                            {turnDetail.cursor_tags.registered.length + turnDetail.cursor_tags.unregistered.length + turnDetail.cursor_tags.leaks.length} tags
                          </span>
                        </div>
                        
                        <table className="w-full text-xs text-left">
                          <thead className="text-slate-500 bg-slate-950/50 border-b border-slate-800">
                            <tr>
                              <th className="px-4 py-2 font-semibold">Tag</th>
                              <th className="px-4 py-2 font-semibold">Type</th>
                              <th className="px-4 py-2 font-semibold">Count</th>
                              <th className="px-4 py-2 font-semibold text-right">Action</th>
                            </tr>
                          </thead>
                          <tbody className="divide-y divide-slate-800">
                            {[
                              ...turnDetail.cursor_tags.leaks.map(t => ({...t, type: 'leak'})),
                              ...turnDetail.cursor_tags.unregistered.map(t => ({...t, type: 'unregistered'})),
                              ...turnDetail.cursor_tags.registered.map(t => ({...t, type: 'registered'}))
                            ].map((tag, idx) => (
                              <tr key={`${tag.tag}-${idx}`} className="hover:bg-slate-800/30 group transition-colors">
                                <td className="px-4 py-2 font-mono text-emerald-300">
                                   &lt;{tag.tag}&gt;
                                </td>
                                <td className="px-4 py-2">
                                  <span className={`inline-flex px-2 py-0.5 rounded text-[10px] uppercase font-bold border ${
                                    tag.type === 'leak' ? 'bg-red-950/40 text-red-400 border-red-900/30' :
                                    tag.type === 'unregistered' ? 'bg-yellow-950/40 text-yellow-400 border-yellow-900/30' :
                                    'bg-emerald-950/40 text-emerald-400 border-emerald-900/30'
                                  }`}>
                                    {tag.type}
                                  </span>
                                </td>
                                <td className="px-4 py-2 font-mono text-slate-400">{tag.count}</td>
                                <td className="px-4 py-2 text-right">
                                   <button 
                                     onClick={() => setSelectedLeakTag(tag)}
                                     className="text-slate-500 hover:text-emerald-400 opacity-0 group-hover:opacity-100 transition-all text-[10px] font-bold uppercase tracking-wide"
                                   >
                                     View Occurrences
                                   </button>
                                </td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      </div>
                    )}

                  {/* Tag Occurrences Panel */}
                  {selectedLeakTag && (
                    <div className="bg-slate-900/50 border border-emerald-800/50 rounded-lg overflow-hidden mb-6 animate-in fade-in slide-in-from-top-2 duration-300">
                      <div className="px-4 py-3 border-b border-emerald-800/50 flex justify-between items-center bg-slate-900/80">
                        <h3 className="text-xs font-bold text-emerald-400 uppercase tracking-widest flex items-center gap-2">
                          <Tag size={14} /> Occurrences of &lt;{selectedLeakTag.tag}&gt;
                        </h3>
                        <button
                          onClick={() => setSelectedLeakTag(null)}
                          className="text-slate-500 hover:text-slate-300 text-lg"
                        >
                          ×
                        </button>
                      </div>
                      <div className="p-4 space-y-2">
                        <div className="text-xs text-slate-400 mb-3">
                          Found <span className="text-emerald-400 font-bold">{selectedLeakTag.count}</span> occurrence{selectedLeakTag.count !== 1 ? 's' : ''} in this turn
                        </div>
                        <div className="space-y-2">
                          {turnDetail.stages.map(stage => (
                            stage.blob_ref && (
                              <button
                                key={stage.name}
                                onClick={() => {
                                  if (selectedCid && selectedTurn) {
                                    const needle = `<${selectedLeakTag.tag}>`;
                                    openBlobStructuredAtNeedle(stage.blob_ref!.blob_id, needle);
                                    setVisibleBlobId(stage.blob_ref!.blob_id);
                                    setSelectedStage(stage);
                                  }
                                }}
                                className="w-full text-left text-xs px-3 py-2 rounded bg-slate-800 hover:bg-slate-700 text-slate-300 hover:text-emerald-300 transition-colors border border-slate-700 hover:border-emerald-600"
                              >
                                <span className="font-mono">{stage.name}</span>
                                <span className="text-slate-500 ml-2">→ View in blob</span>
                              </button>
                            )
                          ))}
                        </div>
                      </div>
                    </div>
                  )}

                  {/* Lifecycle Stages */}
                  <div>
                    <h3 className="text-xs font-bold text-slate-500 uppercase tracking-widest mb-4">Lifecycle Stages</h3>
                    <div className="grid grid-cols-1 gap-4 mb-8">
                      {turnDetail.stages.map(stage => (
                        <div key={stage.name} className="bg-slate-900/50 border border-slate-800 rounded-xl overflow-hidden group hover:border-slate-700 transition-colors">
                          <div className="flex items-center justify-between p-4">
                            <div className="flex items-center gap-3">
                              <div className={`w-2 h-2 rounded-full ${stage.blob_ref ? 'bg-emerald-500' : 'bg-slate-700'}`}></div>
                              <span className="font-mono text-sm font-bold text-slate-200">{stage.name}</span>
                              <span className="text-[10px] uppercase font-bold text-slate-600 bg-slate-800 px-1.5 py-0.5 rounded">{stage.kind}</span>
                            </div>
                            {stage.blob_ref && (
                              <button 
                                onClick={() => {
                                  if (selectedCid && selectedTurn) {
                                    if (visibleBlobId === stage.blob_ref!.blob_id) {
                                      // Hide the blob
                                      setSelectedBlob(null);
                                      setVisibleBlobId(null);
                                      setSelectedStage(null);
                                    } else {
                                      // Show the blob
                                      fetchBlob(selectedCid, selectedTurn, stage.blob_ref!.blob_id);
                                      setVisibleBlobId(stage.blob_ref!.blob_id);
                                      setSelectedStage(stage);
                                    }
                                  }
                                }}
                                className={`text-xs px-4 py-1.5 rounded-lg transition-all shadow-lg font-bold ${
                                  visibleBlobId === stage.blob_ref!.blob_id
                                    ? 'bg-red-600 hover:bg-red-500 text-white shadow-red-900/20'
                                    : 'bg-emerald-600 hover:bg-emerald-500 text-white shadow-emerald-900/20'
                                }`}
                              >
                                {visibleBlobId === stage.blob_ref!.blob_id ? 'Hide Blob' : 'View Blob'}
                              </button>
                            )}
                          </div>
                        </div>
                      ))}
                    </div>
                  </div>

                  {/* Stage Comparison */}
                  <div>
                    <h3 className="text-sm font-bold uppercase text-slate-400 mb-3 flex items-center gap-2">
                      <GitCompare size={16} /> Compare Stages
                    </h3>
                    <div className="grid grid-cols-2 gap-2 mb-3">
                      <select
                        value={compareStages[0] || ''}
                        onChange={(e) => setCompareStages([e.target.value || null, compareStages[1]])}
                        className="text-xs bg-slate-700 border border-slate-600 rounded p-2 text-slate-200"
                      >
                        <option value="">Select stage 1</option>
                        {turnDetail.stages.map(s => (
                          <option key={s.name} value={s.name}>{s.name}</option>
                        ))}
                      </select>
                      <select
                        value={compareStages[1] || ''}
                        onChange={(e) => setCompareStages([compareStages[0], e.target.value || null])}
                        className="text-xs bg-slate-700 border border-slate-600 rounded p-2 text-slate-200"
                      >
                        <option value="">Select stage 2</option>
                        {turnDetail.stages.map(s => (
                          <option key={s.name} value={s.name}>{s.name}</option>
                        ))}
                      </select>
                    </div>
                    {compareStages[0] && compareStages[1] && (
                      <div>
                        <div className="text-xs text-slate-400 p-2 bg-slate-900/50 border border-slate-700 rounded mb-3">
                          Comparing <span className="text-emerald-400 font-mono">{compareStages[0]}</span> vs <span className="text-emerald-400 font-mono">{compareStages[1]}</span>
                        </div>
                        {stageDiffLoading ? (
                          <div className="text-xs text-slate-400 p-3 bg-slate-900/50 border border-slate-700 rounded">
                            Loading diff...
                          </div>
                        ) : stageDiff ? (
                          <div className="bg-slate-900 border border-slate-800 rounded-xl p-4 font-mono text-xs overflow-x-auto text-slate-300 shadow-2xl max-h-[400px] overflow-y-auto">
                            <JsonTreeNode data={stageDiff.diff} />
                          </div>
                        ) : null}
                      </div>
                    )}
                  </div>

                  {/* Blob Viewer */}
                  {selectedBlob && (
                    <div className="animate-in fade-in slide-in-from-bottom-4 duration-300">
                      <div className="flex items-center justify-between mb-4">
                        <h3 className="text-xs font-bold text-slate-500 uppercase tracking-widest">Blob Inspector</h3>
                        <div className="flex gap-2">
                          <button
                            onClick={() => setBlobViewMode('formatted')}
                            className={`text-xs px-3 py-1.5 rounded transition-colors border ${
                              blobViewMode === 'formatted'
                                ? 'bg-emerald-600 border-emerald-500 text-white'
                                : 'bg-slate-700 hover:bg-slate-600 text-slate-200 border-slate-600'
                            }`}
                          >
                            Formatted
                          </button>
                          <button
                            onClick={() => setBlobViewMode('escaped')}
                            className={`text-xs px-3 py-1.5 rounded transition-colors border ${
                              blobViewMode === 'escaped'
                                ? 'bg-emerald-600 border-emerald-500 text-white'
                                : 'bg-slate-700 hover:bg-slate-600 text-slate-200 border-slate-600'
                            }`}
                          >
                            Escaped
                          </button>
                          <button
                            onClick={() => setBlobViewMode('structured')}
                            className={`text-xs px-3 py-1.5 rounded transition-colors border ${
                              blobViewMode === 'structured'
                                ? 'bg-emerald-600 border-emerald-500 text-white'
                                : 'bg-slate-700 hover:bg-slate-600 text-slate-200 border-slate-600'
                            }`}
                          >
                            Structured
                          </button>
                        </div>
                      </div>
                      {blobViewMode === 'structured' ? (
                        <div className="bg-slate-900 border border-slate-800 rounded-xl p-6 font-mono text-xs overflow-x-auto text-slate-300 shadow-2xl min-h-[500px] leading-relaxed max-h-[600px] overflow-y-auto">
                          {parsedBlobJson ? (
                            <JsonTreeNodeControlled
                              data={parsedBlobJson}
                              path=""
                              expandedPaths={expandedJsonPaths}
                              onToggle={(p) => {
                                setExpandedJsonPaths((prev) => {
                                  const next = new Set(prev);
                                  if (next.has(p)) next.delete(p);
                                  else next.add(p);
                                  return next;
                                });
                              }}
                              highlight={blobHighlight}
                            />
                          ) : (
                            <span className="text-red-400">Invalid JSON</span>
                          )}
                        </div>
                      ) : (
                        <div className="bg-slate-900 border border-slate-800 rounded-xl p-6 font-mono text-xs overflow-x-auto whitespace-pre-wrap text-emerald-100 shadow-2xl min-h-[500px] leading-relaxed">
                          {blobViewMode === 'formatted' 
                            ? selectedBlob.content 
                            : JSON.stringify(selectedBlob.content)
                          }
                        </div>
                      )}
                    </div>
                  )}

                  {/* Export Actions */}
                  <div className="flex gap-2 mb-6">
                    <button
                      onClick={() => {
                        if (selectedCid && selectedTurn) {
                          window.location.href = `${API_BASE}/export/turn/${selectedCid}/${selectedTurn}`;
                        }
                      }}
                      className="flex-1 text-xs px-3 py-2 rounded bg-emerald-600 hover:bg-emerald-500 text-white font-bold transition-colors"
                    >
                      Export Turn
                    </button>
                    <button
                      onClick={() => {
                        if (selectedCid) {
                          window.location.href = `${API_BASE}/export/conversation/${selectedCid}`;
                        }
                      }}
                      className="flex-1 text-xs px-3 py-2 rounded bg-slate-700 hover:bg-slate-600 text-slate-200 font-bold transition-colors"
                    >
                      Export Conversation
                    </button>
                    <button
                      onClick={async () => {
                        if (selectedCid && selectedTurn) {
                          try {
                            const res = await axios.post(`${API_BASE}/replay/${selectedCid}/${selectedTurn}`, {
                              stages: ["projected", "final"]
                            });
                            alert(`Replay available: ${res.data.message}`);
                          } catch (err) {
                            console.error("Replay failed", err);
                            alert("Replay not available");
                          }
                        }
                      }}
                      className="flex-1 text-xs px-3 py-2 rounded bg-slate-700 hover:bg-slate-600 text-slate-200 font-bold transition-colors"
                      title="Re-run lift/project stages with stored ingress"
                    >
                      Replay Turn
                    </button>
                  </div>

                  {/* Raw JSON */}
                  <div>
                    <button
                      onClick={() => toggleSummaryExpanded('raw-json')}
                      className="text-xs text-slate-400 hover:text-slate-300 flex items-center gap-1 mb-2"
                    >
                      <ChevronDown
                        size={12}
                        className={`transition-transform ${expandedSummaries.has('raw-json') ? 'rotate-180' : ''}`}
                      />
                      Raw TurnDetail JSON
                    </button>
                    {expandedSummaries.has('raw-json') && (
                      <pre className="text-xs bg-slate-900 border border-slate-700 rounded p-3 overflow-x-auto text-slate-300 max-h-96 overflow-y-auto">
                        {JSON.stringify(turnDetail, null, 2)}
                      </pre>
                    )}
                  </div>
                </div>
              </div>
              </div>
              ) : (
                <div className="h-full flex flex-col items-center justify-center text-slate-500">
                  <Clock size={48} className="mb-4 opacity-20" />
                  <p>Select a turn to see the details</p>
                </div>
              )}
                </Panel>
              </PanelGroup>
            ) : (
              <div className="h-full flex flex-col items-center justify-center text-slate-500 bg-slate-900">
                <Database size={64} className="mb-6 opacity-20" />
                <h2 className="text-2xl font-bold text-slate-400 mb-2">No Conversation Selected</h2>
                <p>Select a conversation from the sidebar to begin debugging</p>
              </div>
            )}
          </div>
        </Panel>
      </PanelGroup>
    </div>
  );
}

export default App;
