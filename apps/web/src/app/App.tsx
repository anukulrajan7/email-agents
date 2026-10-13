import { BrowserRouter } from 'react-router';
import { Providers } from './providers';
import { AppRoutes } from './routes';

export function App() {
  return (
    <Providers>
      <BrowserRouter>
        <AppRoutes />
      </BrowserRouter>
    </Providers>
  );
}
