FROM ruby:3.4-slim

RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends build-essential libsqlite3-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Gemfile Gemfile.lock ./
RUN bundle install
COPY . .

# Sinatra's development-mode host authorization only permits localhost,
# which would 403 in-network requests from the web container.
ENV APP_ENV=production

EXPOSE 9292
CMD ["bundle", "exec", "rackup", "--host", "0.0.0.0", "-p", "9292"]
