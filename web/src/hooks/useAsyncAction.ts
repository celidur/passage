import {useCallback, useState} from 'react';
import {errorMessage} from '../lib/format';

/** Tracks the busy state and error message of a user-triggered async action. */
export function useAsyncAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const run = useCallback(async (action: () => Promise<void>) => {
    setBusy(true);
    setError('');
    try {
      await action();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }, []);

  return {busy, error, run};
}
