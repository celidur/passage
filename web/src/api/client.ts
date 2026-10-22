import type {Route, RouteInput, RouteUpstreams, Status} from './types';

export class UnauthorizedError extends Error {}

export type ApiClient = ReturnType<typeof createClient>;

/** Typed client for the management API. `onUnauthorized` runs whenever the token is rejected. */
export function createClient(token: string, onUnauthorized: (message: string) => void = () => {}) {
  async function request<T>(path: string, method = 'GET', body?: unknown): Promise<T> {
    const response = await fetch('/api' + path, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        ...(body === undefined ? {} : {'Content-Type': 'application/json'}),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (response.ok) {
      return (response.status === 204 ? undefined : await response.json()) as T;
    }
    const message = await readError(response);
    if (response.status === 401) {
      onUnauthorized(message);
      throw new UnauthorizedError(message);
    }
    throw new Error(message);
  }

  const routePath = (id: string) => `/routes/${encodeURIComponent(id)}`;

  return {
    status: () => request<Status>('/status'),
    listRoutes: () => request<Route[]>('/routes'),
    upstreams: () => request<RouteUpstreams[]>('/upstreams'),
    createRoute: (input: RouteInput) => request<Route>('/routes', 'POST', input),
    updateRoute: (id: string, input: RouteInput) => request<Route>(routePath(id), 'PUT', input),
    deleteRoute: (id: string) => request<void>(routePath(id), 'DELETE'),
  };
}

/** API errors are `{"error": "..."}`; request-body rejections are plain text. */
async function readError(response: Response): Promise<string> {
  const text = await response.text();
  try {
    return JSON.parse(text).error ?? text;
  } catch {
    return text || `Request failed (${response.status})`;
  }
}
