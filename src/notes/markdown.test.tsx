import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import Markdown from './markdown';

describe('Markdown', () => {
  it('renders headings at their level', () => {
    render(<Markdown source={'# Title\n## Subtitle\n### Detail'} />);
    expect(screen.getByRole('heading', { level: 1, name: 'Title' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 2, name: 'Subtitle' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 3, name: 'Detail' })).toBeInTheDocument();
  });

  it('renders a bullet list', () => {
    render(<Markdown source={'- first item\n- second item'} />);
    const items = screen.getAllByRole('listitem');
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent('first item');
    expect(items[1]).toHaveTextContent('second item');
  });

  it('renders bold spans inside a strong element', () => {
    render(<Markdown source={'This is **important** context.'} />);
    const strong = screen.getByText('important');
    expect(strong.tagName).toBe('STRONG');
  });

  it('groups consecutive lines into one paragraph', () => {
    render(<Markdown source={'Line one\nline two.\n\nSecond paragraph.'} />);
    expect(screen.getByText('Line one line two.')).toBeInTheDocument();
    expect(screen.getByText('Second paragraph.')).toBeInTheDocument();
  });

  it('renders an empty source without throwing', () => {
    const { container } = render(<Markdown source="" />);
    expect(container).toBeEmptyDOMElement();
  });

  it('escapes an embedded script tag instead of injecting it', () => {
    const { container } = render(
      <Markdown source={'Summary mentions <script>alert("x")</script> in passing.'} />,
    );
    expect(container.querySelector('script')).toBeNull();
    expect(container.textContent).toContain('<script>alert("x")</script>');
  });

  it('escapes an onerror attribute instead of turning it into markup', () => {
    const { container } = render(
      <Markdown source={'A link-like string: <img src=x onerror="alert(1)">'} />,
    );
    expect(container.querySelector('img')).toBeNull();
    expect(container.textContent).toContain('onerror="alert(1)"');
  });
});
