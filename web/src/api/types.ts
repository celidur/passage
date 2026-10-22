/** Fields accepted by `POST /api/routes` and `PUT /api/routes/{id}`. */
export type RouteInput = {
  name: string;
  host: string;
  path_prefix: string;
  upstream: string;
  enabled: boolean;
  forward_auth_url: string | null;
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
