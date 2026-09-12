import type { DiffStats, FileDiffDetail, FileDiffStatus } from '../api/types';

/**
 * unified diff 解析。
 *
 * 后端 `GET /tasks/{id}/files/merge-proposal.diff` 直接返回 diff 文本；
 * DiffStats 由前端从 diff 机械解析（API 无 stage_outputs 读取端点，见报告）。
 */

export type DiffLineKind = 'add' | 'del' | 'ctx' | 'hunk' | 'meta';

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
}

export interface DiffFile {
  path: string;
  status: FileDiffStatus;
  additions: number;
  deletions: number;
  lines: DiffLine[];
}

export interface ParsedDiff {
  files: DiffFile[];
  stats: DiffStats;
}

function pathFromGitHeader(line: string): string | null {
  // diff --git a/foo b/foo
  const m = /^diff --git a\/(.+) b\/(.+)$/.exec(line);
  if (!m) return null;
  return m[2];
}

export function parseUnifiedDiff(text: string): ParsedDiff {
  const files: DiffFile[] = [];
  let current: DiffFile | null = null;
  let pendingStatus: FileDiffStatus = 'modified';

  for (const raw of text.replace(/\r\n/g, '\n').split('\n')) {
    if (raw.startsWith('diff --git ')) {
      const path = pathFromGitHeader(raw);
      current = {
        path: path ?? raw.slice('diff --git '.length).trim(),
        status: 'modified',
        additions: 0,
        deletions: 0,
        lines: [{ kind: 'meta', text: raw }],
      };
      files.push(current);
      pendingStatus = 'modified';
      continue;
    }
    if (!current) {
      // 无 diff --git 头的裸 diff：遇到 ---/+++ 时开一个文件
      if (raw.startsWith('--- ')) {
        current = {
          path: raw.slice(4).trim().replace(/^a\//, ''),
          status: 'modified',
          additions: 0,
          deletions: 0,
          lines: [],
        };
        files.push(current);
      } else {
        continue;
      }
    }

    if (raw.startsWith('new file mode')) {
      pendingStatus = 'added';
      current.status = 'added';
      current.lines.push({ kind: 'meta', text: raw });
      continue;
    }
    if (raw.startsWith('deleted file mode')) {
      pendingStatus = 'deleted';
      current.status = 'deleted';
      current.lines.push({ kind: 'meta', text: raw });
      continue;
    }
    if (raw.startsWith('--- ')) {
      if (raw.slice(4).trim() === '/dev/null') {
        pendingStatus = 'added';
        current.status = 'added';
      }
      continue;
    }
    if (raw.startsWith('+++ ')) {
      const target = raw.slice(4).trim();
      if (target !== '/dev/null') {
        current.path = target.replace(/^b\//, '');
      } else {
        current.status = 'deleted';
      }
      if (current.status === 'modified') current.status = pendingStatus;
      continue;
    }
    if (raw.startsWith('@@')) {
      current.lines.push({ kind: 'hunk', text: raw });
      continue;
    }
    if (raw.startsWith('+')) {
      current.additions += 1;
      current.lines.push({ kind: 'add', text: raw.slice(1) });
      continue;
    }
    if (raw.startsWith('-')) {
      current.deletions += 1;
      current.lines.push({ kind: 'del', text: raw.slice(1) });
      continue;
    }

    current.lines.push({ kind: 'ctx', text: raw.startsWith(' ') ? raw.slice(1) : raw });
  }

  const fileDetails: FileDiffDetail[] = files.map((f) => ({
    path: f.path,
    additions: f.additions,
    deletions: f.deletions,
    status: f.status,
  }));
  const stats: DiffStats = {
    files_changed: files.length,
    insertions: files.reduce((n, f) => n + f.additions, 0),
    deletions: files.reduce((n, f) => n + f.deletions, 0),
    file_details: fileDetails,
  };
  return { files, stats };
}

export function emptyDiffStats(): DiffStats {
  return { files_changed: 0, insertions: 0, deletions: 0, file_details: [] };
}
