import type {Certificate} from '../../api/types';
import {ErrorMessage} from '../../components/Alert';
import {Dialog} from '../../components/Dialog';
import {useAsyncAction} from '../../hooks/useAsyncAction';

type Props = {
  certificate: Certificate;
  onConfirm: () => Promise<void>;
  onClose: () => void;
};

export function DeleteCertificateDialog({certificate, onConfirm, onClose}: Props) {
  const {busy, error, run} = useAsyncAction();
  return (
    <Dialog titleId="delete-certificate-title">
      <h2 id="delete-certificate-title">Delete the certificate for {certificate.domains[0]}?</h2>
      <p>
        Browsers will stop getting it at the next connection. Hosts with HTTPS enabled order a new one the next time
        they are saved.
      </p>
      <ErrorMessage>{error}</ErrorMessage>
      <div className="dialog-foot">
        <button className="secondary" onClick={onClose}>
          Keep certificate
        </button>
        <button className="danger" disabled={busy} onClick={() => void run(onConfirm)}>
          {busy ? 'Deleting…' : 'Delete certificate'}
        </button>
      </div>
    </Dialog>
  );
}
