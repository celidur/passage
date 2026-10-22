import type {Route} from '../../api/types';
import {ErrorMessage} from '../../components/Alert';
import {Dialog} from '../../components/Dialog';
import {useAsyncAction} from '../../hooks/useAsyncAction';

type Props = {
  route: Route;
  onConfirm: () => Promise<void>;
  onClose: () => void;
};

export function DeleteHostDialog({route, onConfirm, onClose}: Props) {
  const {busy, error, run} = useAsyncAction();
  return (
    <Dialog titleId="delete-dialog-title">
      <h2 id="delete-dialog-title">Remove {route.name}?</h2>
      <p>Traffic will stop matching this route immediately.</p>
      <ErrorMessage>{error}</ErrorMessage>
      <div className="dialog-foot">
        <button className="secondary" onClick={onClose}>
          Keep host
        </button>
        <button className="danger" disabled={busy} onClick={() => void run(onConfirm)}>
          {busy ? 'Removing…' : 'Remove host'}
        </button>
      </div>
    </Dialog>
  );
}
