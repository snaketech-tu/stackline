// Mirrors the JSON returned by the swarmdash API (crates/swarmdash-core).

export type Level = 'ERROR' | 'WARN' | 'INFO' | 'DEBUG'

export interface Host {
  id: string; clusterId: string | null; name: string; os: string; kernel: string; dockerVersion: string; apiVersion: string
  ncpu: number; memTotal: number; containersRunning: number; containersTotal: number; images: number
  swarmState: string; isManager: boolean; cpuPercent: number | null; memUsed: number | null
}

export interface Node {
  id: string; hostname: string; role: string; leader: boolean; reachability: string | null
  availability: string; state: string; addr: string; engine: string; os: string
  ncpu: number; memTotal: number; labels: Record<string, string>; isLocal: boolean
  cpuPercent: number | null; memUsed: number | null; tasksRunning: number
}

export interface Task {
  id: string; slot: string; node: string; state: string; desiredState: string
  message: string | null; error: string | null; health: string | null
  containerId: string | null; containerName: string | null; since: number | null
  exitCode: number | null; failures: number; cpuPercent: number | null; memUsed: number | null
}

export interface Service {
  id: string; name: string; fullName: string; stack: string; image: string; mode: string
  desired: number; running: number; ports: string[]; updatedAt: number | null; tty: boolean
  tasks: Task[]; cpuPercent: number | null; memUsed: number | null
}

export interface Stack { name: string; kind: 'swarm' | 'compose' | 'standalone'; services: Service[] }

export interface Snapshot {
  mode: 'swarm' | 'standalone'; host: Host; nodes: Node[]; stacks: Stack[]; fetchedAt: number; warnings: string[]
}

export interface ConnState {
  status: 'connecting' | 'connected' | 'error'; error: string | null; latencyMs: number | null
  endpoint: string | null; updatedAt: number | null; logsSyncedAt: number | null; logErrors: string[]
}

export interface SshFields { target: string; port?: number | null; identity?: string | null; jump?: string | null; socket: string }
export type Connection =
  | ({ id: string; name: string; kind: 'ssh' } & SshFields)
  | { id: string; name: string; kind: 'local'; endpoint?: string | null }

export interface Settings {
  theme: 'system' | 'light' | 'dark'; retentionDays: number; maxStoreMb: number
  refreshSecs: number; logPollSecs: number; collectStats: boolean; checkUpdates: boolean
  alertEnabled: boolean; alertWindowMins: number; alertThreshold: number
}

export interface Meta { version: string; desktop: boolean; storePath: string; platform: string }

export interface Bucket { error: number; warn: number; info: number; debug: number; exc: number }
export interface Group {
  fp: string; pattern: string; count: number; level: Level; first: number; last: number
  services: string[]; nodes: string[]; sources: number; exc: boolean; sample: string; spark: number[]
}
export interface Entry {
  id: number; ts: number; stack: string; service: string; replica: string; node: string; source: string
  stream: 'stdout' | 'stderr'; level: Level; exc: boolean; lines: number; fp: string | null; text: string; raw: string | null
}
export interface LogResult {
  total: number; levels: Record<Level, number>; histogram: Bucket[]; groups: Group[]; entries: Entry[]
}
export interface LogResponse { result: LogResult; syncedAt: number | null; logErrors: string[] }
export interface TestStep { label: string; ok: boolean; detail: string | null; ms: number }
export interface StoreStats {
  bytes: number; entries: number; oldest: number | null
  /** Why the last cleanup of the store failed; it may grow past its limit until one succeeds. */
  maintenanceError?: string | null
}
export interface SnapshotResponse { state: ConnState; snapshot: Snapshot | null; pins: string[]; collect: Collect }
/** Which services get their logs collected: all but the listed ones, or only the listed ones (full names). */
export interface Collect { only: boolean; services: string[] }
export interface ServiceUsage { service: string; entries: number; textBytes: number }
/** Stored lines per service, and the services whose lines are being deleted. */
export interface Usage { services: ServiceUsage[]; deleting: string[] }
export interface NewGroup { fp: string; pattern: string; count: number; service: string; first: number; sample: string }
export interface ErrorSummary {
  byService: Record<string, number>; lastHour: number; prevHour: number; spark: number[]
  /** Oldest stored entry of the connection, µs. */
  oldest: number | null; newGroups: NewGroup[]
}

export type UpdatePhase = 'idle' | 'checking' | 'upToDate' | 'available' | 'downloading' | 'installing' | 'restarting' | 'failed'
export interface Release { version: string; name: string; notes: string; publishedAt: string | null; url: string }
export interface UpdateState {
  current: string; phase: UpdatePhase; releases: Release[]; checkedAt: number | null; error: string | null
  received: number; total: number | null; canInstall: boolean; installHint: string | null; releasesUrl: string
}

export interface EnvVar { name: string; value: string | null }
export interface Mount { kind: string; source: string; target: string; readOnly: boolean }
export interface Network { name: string; ip: string | null; aliases: string[] }
export interface HealthRun { at: number | null; exitCode: number; output: string }
export interface ServiceDetails {
  image: string; created: number | null; updated: number | null; started: number | null
  command: string | null; entrypoint: string | null; workdir: string | null; user: string | null; hostname: string | null
  env: EnvVar[]; labels: [string, string][]; mounts: Mount[]; networks: Network[]; ports: string[]
  restart: string | null; restartCount: number | null; oomKilled: boolean; limits: string | null; reservations: string | null
  healthcheck: string | null; healthLog: HealthRun[]; updateConfig: string | null; rollbackConfig: string | null
  placement: string[]; secrets: string[]; configs: string[]; logDriver: string | null; stopGrace: string | null
  container: string | null
}

export interface Alert {
  conn: string; name: string; active: boolean; count: number; windowMins: number; threshold: number
  /** Unix ms */
  since: number | null; notifiedAt: number | null; seq: number; top: string | null; topService: string | null
}
export interface AlertsResponse { alerts: Alert[]; native: boolean }

/** A service together with the stack it belongs to. */
export interface ServiceRef extends Service { kind: Stack['kind']; key: string }
