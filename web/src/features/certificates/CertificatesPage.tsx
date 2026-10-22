import {useState} from 'react';
import {Lock, Plus, ShieldCheck} from 'lucide-react';
import type {Certificate, Status} from '../../api/types';
import {ErrorMessage, Notice} from '../../components/Alert';
import {errorMessage} from '../../lib/format';
import {CertificateFormDialog} from './CertificateFormDialog';
import {CertificatesTable} from './CertificatesTable';
import {DeleteCertificateDialog} from './DeleteCertificateDialog';
import type {Certificates} from './useCertificates';
import './certificates.css';

type Props = {
  certificates: Certificates;
  https: Status['https'] | undefined;
};

export function CertificatesPage({certificates, https}: Props) {
  const {certificates: list, loadError, create, renew, remove} = certificates;
  const [notice, setNotice] = useState('');
  const [actionError, setActionError] = useState('');
  const [adding, setAdding] = useState(false);
  const [deleting, setDeleting] = useState<Certificate | null>(null);

  const acme = https?.acme ?? false;

  async function requestRenewal(certificate: Certificate) {
    setNotice('');
    setActionError('');
    try {
      await renew(certificate.id);
      setNotice(`A new certificate for ${certificate.domains[0]} is being requested.`);
    } catch (e) {
      setActionError(errorMessage(e));
    }
  }

  return (
    <>
      <div className="title-row">
        <div>
          <div className="eyebrow">HTTPS, ON AUTOPILOT</div>
          <h1>
            Certificates<span className="title-dot">.</span>
          </h1>
          <p>Issued, renewed and swapped in without dropping a connection.</p>
        </div>
        <button className="primary" disabled={!acme} onClick={() => setAdding(true)}>
          <Plus size={17} />
          Add certificate
        </button>
      </div>

      {https && !https.enabled && (
        <div className="warning" role="status">
          <Lock size={16} />
          <p>
            No HTTPS listener is configured. Add a <code>[tls]</code> section to <code>passage.toml</code> to serve HTTPS.
          </p>
        </div>
      )}
      {https?.enabled && !acme && (
        <div className="warning" role="status">
          <Lock size={16} />
          <p>
            Automatic certificates are not configured. Add an <code>[acme]</code> section to <code>passage.toml</code> to
            order and renew them from Let’s Encrypt or another ACME authority.
          </p>
        </div>
      )}
      {notice && <Notice>{notice}</Notice>}
      <ErrorMessage>{loadError || actionError}</ErrorMessage>

      <section className="hosts">
        <div className="table-top">
          <div>
            <h2>
              Your certificates <span>{list.length}</span>
            </h2>
            <p>Renewal starts 30 days before expiry. Enable HTTPS on a host to order its certificate.</p>
          </div>
        </div>
        {list.length ? (
          <CertificatesTable
            certificates={list}
            canRenew={acme}
            onRenew={(certificate) => void requestRenewal(certificate)}
            onDelete={setDeleting}
          />
        ) : (
          <div className="empty">
            <span className="empty-icon">
              <ShieldCheck size={28} />
            </span>
            <h3>No certificates yet</h3>
            <p>Enable HTTPS on a proxy host and its certificate appears here.</p>
          </div>
        )}
        <div className="table-bottom">
          <span>
            <span className="dot" />
            Checked every few seconds
          </span>
          <span>{list.length} certificates</span>
        </div>
      </section>

      {adding && (
        <CertificateFormDialog
          dnsAvailable={https?.dns_01 ?? false}
          onClose={() => setAdding(false)}
          onSave={async (input) => {
            await create(input);
            setAdding(false);
            setNotice('Certificate requested. It appears as valid once the authority has verified the domains.');
          }}
        />
      )}
      {deleting && (
        <DeleteCertificateDialog
          certificate={deleting}
          onClose={() => setDeleting(null)}
          onConfirm={async () => {
            await remove(deleting.id);
            setDeleting(null);
            setNotice('Certificate deleted.');
          }}
        />
      )}
    </>
  );
}
