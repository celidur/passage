import {useCallback, useEffect, useState} from 'react';
import type {ApiClient} from '../../api/client';
import type {Route, RouteInput, Status} from '../../api/types';
import {errorMessage} from '../../lib/format';

const REFRESH_INTERVAL_MS = 10_000;

/** Routes and gateway status, polled in the background and refreshed after every change. */
export function useHosts(client: ApiClient) {
  const [routes, setRoutes] = useState<Route[]>([]);
  const [status, setStatus] = useState<Status | null>(null);
  const [loadError, setLoadError] = useState('');

  const refresh = useCallback(async () => {
    try {
      const [nextStatus, nextRoutes] = await Promise.all([client.status(), client.listRoutes()]);
      setStatus(nextStatus);
      setRoutes(nextRoutes);
      setLoadError('');
    } catch (e) {
      setLoadError(errorMessage(e));
    }
  }, [client]);

  useEffect(() => {
    void refresh();
    const timer = setInterval(refresh, REFRESH_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  const save = useCallback(
    async (id: string | null, input: RouteInput) => {
      if (id) await client.updateRoute(id, input);
      else await client.createRoute(input);
      await refresh();
    },
    [client, refresh],
  );

  const remove = useCallback(
    async (id: string) => {
      await client.deleteRoute(id);
      await refresh();
    },
    [client, refresh],
  );

  return {routes, status, loadError, refresh, save, remove};
}

export type Hosts = ReturnType<typeof useHosts>;
