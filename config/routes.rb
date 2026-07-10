Rails.application.routes.draw do
  devise_for :admins
  get "up" => "rails/health#show", as: :rails_health_check

  namespace :admin do
    root to: "dashboard#index"
    resources :departments, only: [ :index, :new, :create, :destroy ]
    resources :tags, only: [ :index, :create, :destroy ]
    resources :app_clients, except: [ :show ]
    resources :documents, only: [ :index, :new, :create, :destroy ] do
      member { get :download }
    end
    resources :upload_requests, only: [ :index ]
  end
  root to: redirect("/admin")

  namespace :api do
    namespace :v1 do
      get "ping", to: "pings#show"
      resources :documents, only: [ :index, :create, :show, :destroy ] do
        member { get :download }
        resources :tags, only: [ :create ], controller: "document_tags"
      end
      resources :departments, only: [ :index, :create ]
      resources :tags, only: [ :index, :create ]
      resources :upload_requests, only: [ :show ]
    end
  end
end
