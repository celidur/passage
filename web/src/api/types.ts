export type Balancing = 'round_robin' | 'least_connections';
export type UpstreamProtocol = 'http1' | 'auto' | 'http2';
export type CheckKind = 'http' | 'tcp';

export type ActiveCheck = {
  kind: CheckKind;
  path: string;
  interval_secs: number;
  timeout_secs: number;
  healthy_threshold: number;
  unhealthy_threshold: number;
};

/** Everything about a route that is not identity, matching or authentication. */
export type Policy = {
  balancing: Balancing;
  protocol: UpstreamProtocol;
  /** Extra attempts on other upstreams after a connection failure. */
  retries: number;
  timeouts: {connect_secs: number; read_secs: number; write_secs: number};
  limits: {max_connections: number | null; max_request_body_bytes: number | null};
  health: {max_failures: number; cooldown_secs: number; check: ActiveCheck | null};
};

/** Fields accepted by `POST /api/routes` and `PUT /api/routes/{id}`. */
export type RouteInput = {
  name: string;
  host: string;
  path_prefix: string;
  upstreams: string[];
  enabled: boolean;
  forward_auth_url: string | null;
  policy: Policy;
};

export type Route = RouteInput & {id: string};

export type Status = {
  name: string;
  version: string;
  engine: string;
  /** Enabled routes currently in the live snapshot. */
  routes: number;
  requests: number;
  failures: number;
};

export type BackendStatus = {
  upstream: string;
  address: string;
  healthy: boolean;
  in_flight: number;
};

/** Live backends of an enabled route. */
export type RouteUpstreams = {
  route_id: string;
  /** Requests currently admitted for the route. */
  active: number;
  backends: BackendStatus[];
};
