import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import App from './App';

describe('App', () => {
  it('renders the app name', () => {
    render(<App />);
    expect(screen.getByRole('heading', { name: 'Resumeira' })).toBeInTheDocument();
  });

  it('tells the user where recording starts', () => {
    render(<App />);
    expect(screen.getByText(/tray icon/i)).toBeInTheDocument();
  });
});
