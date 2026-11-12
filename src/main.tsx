import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { isTauri } from './core/client';
import './styles/globals.css';

async function boot(): Promise<void> {
  let hasCore = isTauri();
  // Only a build in Vite mode `mock` sets the flag, so every other build drops this branch and
  // the mocked core with it (REQ-UI-17).
  if (import.meta.env.VITE_MOCK_CORE === '1' && !hasCore) {
    const { installMockCore } = await import('./core/mock');
    installMockCore(new URL(window.location.href).searchParams);
    hasCore = true;
  }
  // The app loads only after the mocks exist, so nothing reads the core or the window label
  // before they do.
  const { App } = await import('./App');
  const root = document.getElementById('root');
  if (root !== null) {
    createRoot(root).render(
      <StrictMode>
        <App hasCore={hasCore} />
      </StrictMode>,
    );
  }
}

void boot();
