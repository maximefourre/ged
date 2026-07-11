Rails.application.routes.draw do
  devise_for :admins, controllers: { sessions: "admins/sessions" }
  get "up" => "rails/health#show", as: :rails_health_check

  namespace :admin do
    root to: "dashboard#index"
    resources :departments, only: [ :index, :new, :create, :edit, :update, :destroy ]
    resources :app_clients, except: [ :show ]
    resources :documents, only: [ :index, :new, :create, :edit, :update, :destroy ] do
      member { get :download }
    end
    resources :upload_requests, only: [ :index ]
    resource :upload_queue_pause, only: [ :create, :destroy ]
  end
  root to: redirect("/admin")

  namespace :api do
    namespace :v1 do
      get "ping", to: "pings#show"
      resources :documents, only: [ :index, :create, :show, :destroy ] do
        member { get :download }
        resource :tags, only: [ :update ], controller: "document_tags"
      end
      resources :departments, only: [ :index, :create ]
      resources :upload_requests, only: [ :show ]
    end
  end
end
