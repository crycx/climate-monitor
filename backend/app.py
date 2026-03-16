import os
from pathlib import Path

from flask import Flask, Response, send_file
from flask_sqlalchemy import SQLAlchemy
from flask_jwt_extended import JWTManager

app = Flask(__name__)


def _openapi_spec_path() -> Path:
    configured = os.getenv("OPENAPI_SPEC_PATH")
    if configured:
        return Path(configured)
    candidate = Path(app.root_path) / "swagger.yaml"
    if candidate.exists() and candidate.stat().st_size > 0:
        return candidate

    repo_root_candidate = Path(app.root_path).parent / "swagger.yaml"
    if repo_root_candidate.exists():
        return repo_root_candidate

    return candidate

@app.route("/")
def hello():
    return "This is climate monitor"


@app.get("/openapi.yaml")
def openapi_yaml():
        spec_path = _openapi_spec_path()
        if not spec_path.exists():
                return {"error": f"OpenAPI spec not found at {spec_path}"}, 404
        return send_file(spec_path, mimetype="application/yaml")


@app.get("/swagger")
@app.get("/swagger/")
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