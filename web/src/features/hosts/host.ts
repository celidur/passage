import type {ActiveCheck, Policy, Route, RouteInput} from '../../api/types';

/** Editable form state; an empty forward-auth URL means "public", upstreams are one per line. */
export type HostForm = Omit<RouteInput, 'forward_auth_url' | 'upstreams'> & {
  forward_auth_url: string;
  upstreams: string;
};

/** Mirrors the server defaults, so a new host shows what it will get. */
export const DEFAULT_POLICY: Policy = {
  balancing: 'round_robin',
  protocol: 'http1',
  retries: 2,
  timeouts: {connect_secs: 5, read_secs: 60, write_secs: 60},
  limits: {max_connections: null, max_request_body_bytes: null},
  health: {max_failures: 3, cooldown_secs: 10, check: null},
};

export const DEFAULT_CHECK: ActiveCheck = {
  kind: 'http',
  path: '/',
  interval_secs: 10,
  timeout_secs: 3,
  healthy_threshold: 2,
  unhealthy_threshold: 2,
};

export const EMPTY_HOST_FORM: HostForm = {
  name: '',
  host: '',
  path_prefix: '/',
  upstreams: 'http://127.0.0.1:3000',
  enabled: true,
  forward_auth_url: '',
  https: {enabled: false, redirect: false},
  policy: DEFAULT_POLICY,
};

export function toForm(route: Route): HostForm {
  const {id: _id, ...input} = route;
  return {
    ...input,
    upstreams: input.upstreams.join('\n'),
    forward_auth_url: input.forward_auth_url ?? '',
  };
}

export function toInput(form: HostForm): RouteInput {
  return {
    ...form,
    upstreams: form.upstreams
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean),
    forward_auth_url: form.forward_auth_url.trim() || null,
  };
}

export const isProtected = (route: Route) => Boolean(route.forward_auth_url);

export function matchesQuery(route: Route, query: string) {
  const haystack = `${route.name} ${route.host} ${route.upstreams.join(' ')}`.toLowerCase();
  return haystack.includes(query.toLowerCase());
}
