FROM python:3.12-alpine
WORKDIR /app
RUN apk add --no-cache gcc musl-dev linux-headers
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY . .

ENV FLASK_APP=run.py
EXPOSE 5000
CMD ["flask", "run", "--host=0.0.0.0"]