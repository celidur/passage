import {useCallback, useEffect, useState} from 'react';
import type {ApiClient} from '../../api/client';
import type {Certificate, NewCertificate} from '../../api/types';
import {errorMessage} from '../../lib/format';

/** Issuance takes seconds, so certificates are polled faster than hosts. */
const REFRESH_INTERVAL_MS = 5_000;

export function useCertificates(client: ApiClient) {
  const [certificates, setCertificates] = useState<Certificate[]>([]);
  const [loadError, setLoadError] = useState('');

  const refresh = useCallback(async () => {
    try {
      setCertificates(await client.listCertificates());
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

  const create = useCallback(
    async (input: NewCertificate) => {
      await client.createCertificate(input);
      await refresh();
    },
    [client, refresh],
  );

  const renew = useCallback(
    async (id: string) => {
      await client.renewCertificate(id);
      await refresh();
    },
    [client, refresh],
  );

  const remove = useCallback(
    async (id: string) => {
      await client.deleteCertificate(id);
      await refresh();
    },
    [client, refresh],
  );

  return {certificates, loadError, refresh, create, renew, remove};
}

export type Certificates = ReturnType<typeof useCertificates>;
