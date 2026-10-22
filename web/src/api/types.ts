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

/** HTTPS for a host: an automatic certificate, and optionally a redirect from plain HTTP. */
export type Https = {enabled: boolean; redirect: boolean};

/** Fields accepted by `POST /api/routes` and `PUT /api/routes/{id}`. */
export type RouteInput = {
  name: string;
  host: string;
  path_prefix: string;
  upstreams: string[];
  enabled: boolean;
  forward_auth_url: string | null;
  https: Https;
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
  /** What this server can do about HTTPS, as configured. */
  https: {enabled: boolean; acme: boolean; dns_01: boolean};
};

export type CertificateChallenge = 'http-01' | 'dns-01';
export type CertificateStatus = 'pending' | 'issuing' | 'valid' | 'expired' | 'failed';

/** A managed certificate. Times are Unix seconds. */
export type Certificate = {
  id: string;
  domains: string[];
  challenge: CertificateChallenge;
  status: CertificateStatus;
  created_at: number;
  not_before: number | null;
  not_after: number | null;
  /** The latest failure; also set while a renewal fails and the current certificate still serves. */
  last_error: string | null;
  failures: number;
  next_attempt: number | null;
};

export type NewCertificate = {domains: string[]; challenge: CertificateChallenge};

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
