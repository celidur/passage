import {RefreshCw, Trash2} from 'lucide-react';
import type {Certificate} from '../../api/types';
import {STATUS_LABEL, dateText, expiryText, isAtRisk, retryText, tone} from './certificate';

type Props = {
  certificates: Certificate[];
  canRenew: boolean;
  onRenew: (certificate: Certificate) => void;
  onDelete: (certificate: Certificate) => void;
};

export function CertificatesTable({certificates, canRenew, onRenew, onDelete}: Props) {
  return (
    <div className="table-scroll">
      <table>
        <thead>
          <tr>
            <th>DOMAINS</th>
            <th>VALIDATION</th>
            <th>STATUS</th>
            <th>EXPIRY</th>
            <th>
              <span className="sr-only">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {certificates.map((certificate) => (
            <CertificateRow
              key={certificate.id}
              certificate={certificate}
              canRenew={canRenew}
              onRenew={onRenew}
              onDelete={onDelete}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}

function CertificateRow({certificate, canRenew, onRenew, onDelete}: Omit<Props, 'certificates'> & {certificate: Certificate}) {
  const [first, ...others] = certificate.domains;
  const atRisk = isAtRisk(certificate);
  return (
    <tr>
      <td>
        <div className="service">
          <div>
            <strong>{first}</strong>
            {others.length > 0 && <small>{others.join(', ')}</small>}
          </div>
        </div>
      </td>
      <td>
        <span className="badge">{certificate.challenge === 'dns-01' ? 'DNS-01' : 'HTTP-01'}</span>
      </td>
      <td>
        <span className={`cert-status ${tone(certificate)}`}>
          <span className="dot" />
          {STATUS_LABEL[certificate.status]}
        </span>
        {certificate.last_error && (
          <small className="cert-error" role="alert">
            {certificate.last_error}
            {certificate.next_attempt !== null && <span> {retryText(certificate)}</span>}
          </small>
        )}
      </td>
      <td>
        {certificate.not_after === null ? (
          <small>—</small>
        ) : (
          <>
            <span className={atRisk ? 'expiry at-risk' : 'expiry'}>{expiryText(certificate)}</span>
            <small className="expiry-date">{dateText(certificate.not_after)}</small>
          </>
        )}
      </td>
      <td>
        <div className="actions">
          <button
            className="icon"
            aria-label={`Renew ${first} now`}
            title="Request a new certificate now"
            disabled={!canRenew || certificate.status === 'issuing'}
            onClick={() => onRenew(certificate)}
          >
            <RefreshCw size={15} />
          </button>
          <button className="icon" aria-label={`Delete certificate for ${first}`} onClick={() => onDelete(certificate)}>
            <Trash2 size={15} />
          </button>
        </div>
      </td>
    </tr>
  );
}
