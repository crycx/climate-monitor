import os
from pathlib import Path

from flask import Blueprint, Response, current_app, send_file


docs_bp = Blueprint("docs", __name__)


def _openapi_spec_path() -> Path:
    configured = os.getenv("OPENAPI_SPEC_PATH")
    if configured:
        return Path(configured)

    root = Path(current_app.root_path)
    candidate = root.parent / "swagger.yaml"
    if candidate.exists() and candidate.stat().st_size > 0:
        return candidate

    repo_root_candidate = root.parent.parent / "swagger.yaml"
    if repo_root_candidate.exists():
        return repo_root_candidate

    return candidate


@docs_bp.get("/")
def hello():
    return "This is climate monitor"


@docs_bp.get("/openapi.yaml")
def openapi_yaml():
    spec_path = _openapi_spec_path()
    if not spec_path.exists():
        return {"error": f"OpenAPI spec not found at {spec_path}"}, 404
    return send_file(spec_path, mimetype="application/yaml")


@docs_bp.get("/swagger")
@docs_bp.get("/swagger/")
def swagger_ui():
    html = """<!doctype html>
<html lang=\"en\">
    <head>
        <meta charset=\"utf-8\" />
        <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
        <title>Climate Monitor API Docs</title>
        <link rel=\"stylesheet\" href=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui.css\" />
    </head>
    <body>
        <div id=\"swagger-ui\"></div>
        <script src=\"https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js\"></script>
        <script>
            window.onload = () => {
                SwaggerUIBundle({
                    url: '/openapi.yaml',
                    dom_id: '#swagger-ui',
                });
            };
        </script>
    </body>
</html>
"""
    return Response(html, mimetype="text/html")
