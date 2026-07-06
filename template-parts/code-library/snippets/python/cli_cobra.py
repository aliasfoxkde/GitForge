"""CLI application structure using Cobra framework.

Builds a production-ready command-line interface with subcommands,
flags, and environment variable support.

Usage:
    # Create commands
    root = RootCommand("myapp", "1.0.0")
    root.AddCommand(serveCommand())
    root.AddCommand(configCommand())
    root.Execute()

Dependencies:
    pip install click
"""

from __future__ import annotations

import sys
from typing import Any, Callable, Optional

import click


# ─── Base Command Setup ────────────────────────────────────────────────────────


class BaseCommand(click.Group):
    """Extended click.Group with common patterns.

    Features:
    - Version information
    - Debug mode
    - Structured error handling
    - Environment variable prefix
    """

    def __init__(
        self,
        name: str,
        help_text: str,
        version: str = "1.0.0",
        **kwargs: Any,
    ) -> None:
        super().__init__(name=name, **kwargs)
        self.help_text = help_text
        self._version = version
        self._debug = False

    def main(self, ctx: click.Context, **kwargs: Any) -> Any:
        """Override main to handle debug mode and errors."""
        if self._debug or ctx.params.get("debug", False):
            import logging
            logging.basicConfig(level=logging.DEBUG)

        try:
            return super().main(ctx, **kwargs)
        except Exception as e:
            if self._debug:
                raise
            click.echo(f"Error: {e}", err=True)
            sys.exit(1)


# ─── Decorator Helpers ────────────────────────────────────────────────────────


def command(
    name: str,
    help_text: str = "",
    **kwargs: Any,
) -> Callable[[Callable[..., Any]], click.Command]:
    """Create a command with common defaults."""
    def decorator(fn: Callable[..., Any]) -> click.Command:
        return click.command(
            name=name,
            help=help_text,
            **kwargs,
        )(fn)
    return decorator


def group(name: str, help_text: str = "") -> Callable[[Callable[..., Any]], click.Group]:
    """Create a command group with common defaults."""
    def decorator(fn: Callable[..., Any]) -> click.Group:
        return click.group(
            name=name,
            help=help_text,
        )(fn)
    return decorator


def option(*args: Any, **kwargs: Any) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    """Add a flag/option to a command."""
    return click.option(*args, **kwargs)


def argument(*args: Any, **kwargs: Any) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    """Add an argument to a command."""
    return click.argument(*args, **kwargs)


# ─── Common Options ───────────────────────────────────────────────────────────


verbose_option = click.option(
    "-v", "--verbose",
    count=True,
    help="Increase verbosity (-v info, -vv debug)",
)

debug_option = click.option(
    "-d", "--debug",
    is_flag=True,
    help="Enable debug mode",
)

output_option = click.option(
    "-o", "--output",
    type=click.Choice(["text", "json", "yaml"]),
    default="text",
    help="Output format",
)

config_option = click.option(
    "-c", "--config",
    type=click.Path(exists=True),
    help="Path to config file",
)


# ─── Shared Arguments ────────────────────────────────────────────────────────


resource_arg = click.argument("resource", help="Resource name or path")
id_arg = click.argument("id", help="Resource ID")
name_arg = click.argument("name", help="Resource name")


# ─── Output Formatters ────────────────────────────────────────────────────────


def echo_success(message: str) -> None:
    """Print a success message in green."""
    click.secho(f"Success: {message}", fg="green")


def echo_error(message: str) -> None:
    """Print an error message in red."""
    click.secho(f"Error: {message}", fg="red", err=True)


def echo_warning(message: str) -> None:
    """Print a warning message in yellow."""
    click.secho(f"Warning: {message}", fg="yellow")


def echo_info(message: str) -> None:
    """Print an info message."""
    click.echo(message)


# ─── Example Commands ────────────────────────────────────────────────────────


@click.group(name="app")
@click.version_option(version="1.0.0", prog_name="myapp")
def cli() -> None:
    """MyApp CLI - A sample application.

    For help on a specific command: myapp <command> --help
    """
    pass


@cli.command()
@click.option("--host", default="localhost", help="Server host")
@click.option("--port", default=8080, help="Server port")
@debug_option
def serve(host: str, port: int, debug: bool) -> None:
    """Start the HTTP server."""
    echo_info(f"Starting server on {host}:{port}")
    if debug:
        echo_warning("Debug mode enabled")


@cli.command()
@config_option
@output_option
def status(config: Optional[str], output: str) -> None:
    """Check application status."""
    import json

    data = {
        "status": "healthy",
        "version": "1.0.0",
    }

    if output == "json":
        click.echo(json.dumps(data, indent=2))
    else:
        echo_success(f"Status: {data['status']}")
        click.echo(f"Version: {data['version']}")


@cli.group(name="config")
def config_group() -> None:
    """Configuration management commands."""
    pass


@config_group.command("get")
@click.argument("key")
def config_get(key: str) -> None:
    """Get a configuration value."""
    # Example: read from config store
    click.echo(f"{key}=<value>")


@config_group.command("set")
@click.argument("key")
@click.argument("value")
def config_set(key: str, value: str) -> None:
    """Set a configuration value."""
    echo_success(f"Set {key}={value}")


# ─── Entry Point ──────────────────────────────────────────────────────────────


def main() -> None:
    """Entry point for the CLI."""
    cli()


if __name__ == "__main__":
    main()
