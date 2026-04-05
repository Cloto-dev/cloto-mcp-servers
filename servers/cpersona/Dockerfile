FROM python:3.13-slim

WORKDIR /app

# Copy common utilities (mcp_utils, validation)
COPY servers/common/ ./common/

# Copy CPersona server
COPY servers/cpersona/ ./cpersona/

# Install dependencies
RUN pip install --no-cache-dir ./cpersona

# Default environment
ENV CPERSONA_DB_PATH=/data/cpersona.db

# Expose Streamable HTTP port (optional)
EXPOSE 8400

CMD ["python", "cpersona/server.py"]
